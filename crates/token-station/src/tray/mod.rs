//! `token-station tray`: a `StatusNotifierItem` for desktops with no native front end.
//!
//! The tray owns no data. It follows the daemon's snapshot and the desktop colour
//! scheme, and redraws the meter only when one of those actually changes.

pub mod client;
pub mod icon;
pub mod menu;
pub mod palette;
pub mod portal;

use std::time::Duration;

use anyhow::Context;
use ksni::{Category, Status, ToolTip, TrayMethods};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use ts_core::{Level, ProviderState, Snapshot};
use zbus::fdo::{RequestNameFlags, RequestNameReply};

use crate::clock::{Clock, system_clock};
use crate::tray::client::Update;
use crate::tray::icon::BarSpec;
use crate::tray::palette::ColorScheme;

/// Item id, stable across sessions.
pub const ITEM_ID: &str = "token-station";
/// Name the tray owns, so a second one exits instead of stacking up a
/// second icon: `setup` starts a tray and the autostart entry starts one too.
pub const TRAY_BUS_NAME: &str = "dev.soldunov.TokenStation.Tray";
/// Object `uninstall` calls to ask the tray to quit.
pub const TRAY_OBJECT_PATH: &str = "/dev/soldunov/TokenStation/Tray";
/// Its interface; internal, and not part of `docs/dbus-api.md`.
pub const TRAY_INTERFACE_NAME: &str = "dev.soldunov.TokenStation.Tray1";
/// How often the countdowns in the tooltip and the menu are refreshed.
const TICK: Duration = Duration::from_secs(60);

/// What a menu item asks the run loop to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Refresh,
    Quit,
}

/// The `StatusNotifierItem`.
pub struct TokenStationTray {
    snapshot: Option<Box<Snapshot>>,
    scheme: ColorScheme,
    /// Pixmaps for the bars and scheme in `rendered`.
    icons: Vec<icon::Pixmap>,
    /// What `icons` was drawn from, so an unchanged meter costs no redraw.
    rendered: (Vec<BarSpec>, ColorScheme),
    actions: UnboundedSender<Action>,
    clock: Clock,
}

impl TokenStationTray {
    /// A tray showing `snapshot`, with its icons already drawn.
    pub fn new(
        snapshot: Option<Box<Snapshot>>,
        scheme: ColorScheme,
        actions: UnboundedSender<Action>,
        clock: Clock,
    ) -> TokenStationTray {
        let mut tray = TokenStationTray {
            snapshot,
            scheme,
            icons: Vec::new(),
            rendered: (Vec::new(), scheme),
            actions,
            clock,
        };
        tray.redraw();
        tray
    }

    /// Bars the current snapshot asks for.
    fn specs(&self) -> Vec<BarSpec> {
        icon::bar_specs(self.snapshot.as_ref().map(|s| &s.meter))
    }

    /// Draw the pixmaps, unconditionally.
    fn redraw(&mut self) {
        let specs = self.specs();
        self.icons = icon::render_all(&specs, self.scheme);
        self.rendered = (specs, self.scheme);
    }

    /// Draw only when the meter or the scheme moved.
    fn redraw_if_stale(&mut self) {
        if self.rendered != (self.specs(), self.scheme) {
            self.redraw();
        }
    }

    /// Apply one watcher update.
    pub fn apply(&mut self, update: Update) {
        match update {
            Update::Daemon(snapshot) => self.snapshot = snapshot,
            Update::Scheme(scheme) => self.scheme = scheme,
        }
        self.redraw_if_stale();
    }

    /// Does any provider carry usable numbers?
    fn has_data(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.providers.iter().any(|provider| {
                matches!(provider.state, ProviderState::Ok | ProviderState::Stale)
                    && !provider.windows.is_empty()
            })
        })
    }

    /// Worst level across the meter bars.
    fn level(&self) -> Level {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.meter.level)
            .unwrap_or_default()
    }

    /// One disabled label item.
    fn label(text: &str) -> ksni::MenuItem<Self> {
        ksni::menu::StandardItem {
            // A lone underscore would be read as a mnemonic by the menu host.
            label: text.replace('_', "__"),
            enabled: false,
            ..Default::default()
        }
        .into()
    }

    /// One item that posts `action` and returns immediately.
    fn command(text: &str, action: Action) -> ksni::MenuItem<Self> {
        ksni::menu::StandardItem {
            label: text.into(),
            activate: Box::new(move |tray: &mut Self| {
                let _ = tray.actions.send(action);
            }),
            ..Default::default()
        }
        .into()
    }
}

impl ksni::Tray for TokenStationTray {
    /// The item is a menu: hosts open it instead of sending `Activate`.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        ITEM_ID.into()
    }

    fn title(&self) -> String {
        menu::TITLE.into()
    }

    fn category(&self) -> Category {
        Category::ApplicationStatus
    }

    fn status(&self) -> Status {
        match (self.level(), self.has_data()) {
            (Level::Critical, _) => Status::NeedsAttention,
            (_, false) => Status::Passive,
            _ => Status::Active,
        }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icons.iter().map(ksni::Icon::from).collect()
    }

    /// Hosts draw this one in `NeedsAttention`; without it the meter vanishes
    /// exactly when it matters most.
    fn attention_icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon_pixmap()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: menu::TITLE.into(),
            description: menu::tooltip_description(self.snapshot.as_deref(), (self.clock)()),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let text = menu::menu_text(self.snapshot.as_deref(), (self.clock)());
        let mut items: Vec<ksni::MenuItem<Self>> =
            text.usage.iter().map(|line| Self::label(line)).collect();
        if !text.tokens.is_empty() {
            items.push(ksni::MenuItem::Separator);
            items.extend(text.tokens.iter().map(|line| Self::label(line)));
        }
        items.push(ksni::MenuItem::Separator);
        items.push(Self::command("Refresh", Action::Refresh));
        items.push(Self::command("Quit", Action::Quit));
        items
    }
}

/// The tray's own tiny interface: one method, so `uninstall` can stop it.
pub struct TrayControl {
    actions: UnboundedSender<Action>,
}

#[zbus::interface(name = "dev.soldunov.TokenStation.Tray1")]
impl TrayControl {
    /// Leave the run loop, exactly as the "Quit" menu item does.
    fn quit(&self) {
        let _ = self.actions.send(Action::Quit);
    }
}

/// Serve the control object, then take the name. `false` when a tray already has it.
async fn claim_tray_name(
    connection: &zbus::Connection,
    actions: UnboundedSender<Action>,
) -> anyhow::Result<bool> {
    connection
        .object_server()
        .at(TRAY_OBJECT_PATH, TrayControl { actions })
        .await?;
    let reply = connection
        .request_name_with_flags(TRAY_BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await;
    match reply {
        Ok(RequestNameReply::PrimaryOwner) => Ok(true),
        Ok(_) | Err(zbus::Error::NameTaken) => Ok(false),
        Err(error) => Err(anyhow::Error::new(error).context("cannot request the tray's bus name")),
    }
}

/// Run the tray until "Quit" or a fatal bus error.
///
/// # Errors
///
/// Returns an error when there is no session bus, when asking for the tray's own
/// bus name fails, or when the `StatusNotifierItem` cannot be published. Finding
/// another tray already running is not an error: this one returns instead.
pub async fn run() -> anyhow::Result<()> {
    let connection = zbus::Connection::session()
        .await
        .context("no session bus; the tray needs a desktop session")?;

    let (updates, mut inbox) = unbounded_channel();
    let (actions, mut commands) = unbounded_channel();
    if !claim_tray_name(&connection, actions.clone()).await? {
        tracing::info!("a Token Station tray is already running; leaving it to it");
        return Ok(());
    }

    let scheme = portal::read_scheme(&connection).await.unwrap_or_default();
    let snapshot = client::initial_snapshot(&connection).await;
    // The watcher may register after us — an autostarted tray usually beats the
    // panel to the bus — so do not treat its absence as fatal.
    let handle = TokenStationTray::new(snapshot, scheme, actions, system_clock())
        .assume_sni_available(true)
        .spawn()
        .await
        .context("cannot publish the StatusNotifierItem on this session bus")?;

    tokio::spawn(client::watch_daemon(connection.clone(), updates.clone()));
    tokio::spawn(client::watch_scheme(connection.clone(), updates));

    let mut ticker = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            Some(update) = inbox.recv() => {
                handle.update(|tray| tray.apply(update)).await;
            }
            Some(action) = commands.recv() => {
                if action == Action::Quit {
                    break;
                }
                spawn_refresh(connection.clone());
            }
            // Keeps "resets in …" honest between snapshots; the icons are cached.
            _ = ticker.tick() => { handle.update(|_| ()).await; }
            else => break,
        }
    }
    handle.shutdown().await;
    Ok(())
}

/// Refresh off the run loop, so a slow daemon never freezes the menu.
fn spawn_refresh(connection: zbus::Connection) {
    tokio::spawn(async move {
        if let Err(error) = client::request_refresh(&connection).await {
            tracing::warn!(%error, "refresh failed");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::fixed_clock;
    use ksni::Tray;
    use tokio::sync::mpsc::UnboundedReceiver;

    const NOW: i64 = 1_790_596_800;

    fn fixture(name: &str) -> Snapshot {
        let json = std::fs::read_to_string(format!(
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/fixtures/{}.json"),
            name
        ))
        .expect("fixture reads");
        serde_json::from_str(&json).expect("fixture parses")
    }

    fn tray(name: Option<&str>) -> (TokenStationTray, UnboundedReceiver<Action>) {
        let (actions, rx) = unbounded_channel();
        let snapshot = name.map(|name| Box::new(fixture(name)));
        (
            TokenStationTray::new(snapshot, ColorScheme::Light, actions, fixed_clock(NOW)),
            rx,
        )
    }

    #[test]
    fn the_item_identifies_itself_the_way_hosts_expect() {
        let (tray, _rx) = tray(Some("snapshot-ok"));
        assert_eq!(tray.id(), "token-station");
        assert_eq!(tray.title(), "Token Station");
        assert_eq!(tray.category(), Category::ApplicationStatus);
        let item_is_menu = TokenStationTray::MENU_ON_ACTIVATE;
        assert!(item_is_menu, "the item is a menu (SNI `ItemIsMenu`)");
        assert_eq!(tray.icon_pixmap().len(), icon::SIZES.len());
    }

    #[test]
    fn needs_attention_does_not_blank_the_icon() {
        // Hosts draw AttentionIconPixmap in that state, so it has to carry the meter.
        let (critical, _rx) = tray(Some("snapshot-near-limit"));
        assert_eq!(critical.status(), Status::NeedsAttention);
        assert_eq!(
            critical.attention_icon_pixmap().len(),
            critical.icon_pixmap().len()
        );
        assert!(!critical.attention_icon_pixmap().is_empty());
    }

    #[test]
    fn the_status_follows_the_worst_level() {
        assert_eq!(tray(Some("snapshot-ok")).0.status(), Status::Active);
        assert_eq!(
            tray(Some("snapshot-near-limit")).0.status(),
            Status::NeedsAttention
        );
        // Nothing to show yet, and nothing at all.
        assert_eq!(tray(Some("snapshot-loading")).0.status(), Status::Passive);
        assert_eq!(tray(None).0.status(), Status::Passive);
    }

    #[test]
    fn the_tooltip_and_menu_come_from_the_snapshot() {
        let (tray, _rx) = tray(Some("snapshot-ok"));
        let tip = tray.tool_tip();
        assert_eq!(tip.title, "Token Station");
        assert!(
            tip.description.starts_with("Claude Code · Session 34%"),
            "{tip:?}"
        );
        // headings + windows, separator, two token lines, separator, Refresh, Quit
        assert_eq!(tray.menu().len(), 6 + 1 + 2 + 1 + 2);
    }

    #[test]
    fn without_a_daemon_the_menu_says_so_and_still_offers_the_commands() {
        let (tray, _rx) = tray(None);
        assert_eq!(tray.tool_tip().description, menu::DAEMON_OFFLINE);
        // one label, separator, Refresh, Quit
        assert_eq!(tray.menu().len(), 4);
    }

    #[test]
    fn a_new_snapshot_with_the_same_meter_does_not_redraw() {
        let (mut tray, _rx) = tray(Some("snapshot-ok"));
        let before = tray.icons.clone();
        let rendered = tray.rendered.clone();

        // Same meter, different token counts: the icon must not move.
        let mut same_meter = fixture("snapshot-ok");
        same_meter.revision = 99;
        tray.apply(Update::Daemon(Some(Box::new(same_meter))));
        assert_eq!(tray.rendered, rendered);
        assert_eq!(tray.icons, before);

        // A different meter redraws.
        tray.apply(Update::Daemon(Some(Box::new(fixture(
            "snapshot-near-limit",
        )))));
        assert_ne!(tray.icons, before);
    }

    #[test]
    fn switching_the_colour_scheme_redraws() {
        let (mut tray, _rx) = tray(Some("snapshot-ok"));
        let before = tray.icons.clone();
        tray.apply(Update::Scheme(ColorScheme::Dark));
        assert_ne!(tray.icons, before);
        assert_eq!(tray.rendered.1, ColorScheme::Dark);
        // Applying the same scheme again is a no-op.
        let after = tray.icons.clone();
        tray.apply(Update::Scheme(ColorScheme::Dark));
        assert_eq!(tray.icons, after);
    }

    #[test]
    fn losing_the_daemon_falls_back_to_the_empty_meter() {
        let (mut tray, _rx) = tray(Some("snapshot-ok"));
        tray.apply(Update::Daemon(None));
        assert_eq!(tray.status(), Status::Passive);
        assert_eq!(tray.rendered.0.len(), 2);
        assert!(tray.rendered.0.iter().all(|bar| bar.percent.is_none()));
    }

    #[test]
    fn underscores_in_labels_are_not_read_as_mnemonics() {
        let mut snapshot = fixture("snapshot-ok");
        snapshot.providers[0].name = "Claude_Code".into();
        let (actions, _rx) = unbounded_channel();
        let tray = TokenStationTray::new(
            Some(Box::new(snapshot)),
            ColorScheme::Light,
            actions,
            fixed_clock(NOW),
        );
        let labels = menu::menu_text(tray.snapshot.as_deref(), NOW);
        assert!(labels.usage[0].contains("Claude_Code"));
        assert_eq!(
            TokenStationTray::label(&labels.usage[0]).label_for_test(),
            "Claude__Code · Max 20x"
        );
    }

    /// Menu items keep their fields private, so reach the label the same way
    /// the host would: through the item the tray built.
    trait LabelForTest {
        fn label_for_test(&self) -> String;
    }

    impl LabelForTest for ksni::MenuItem<TokenStationTray> {
        fn label_for_test(&self) -> String {
            match self {
                ksni::MenuItem::Standard(item) => item.label.clone(),
                _ => String::new(),
            }
        }
    }
}
