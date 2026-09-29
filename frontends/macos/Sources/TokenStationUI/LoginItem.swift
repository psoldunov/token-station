import Foundation
import Observation
import ServiceManagement

/// "Open at Login", through the `SMAppService` registration macOS 13 introduced.
///
/// macOS can hold the registration until the user approves it in Login Items;
/// when it does, the switch says so instead of quietly snapping back.
@Observable
@MainActor
final class LoginItemController {
    private(set) var isEnabled = false
    private(set) var needsApproval = false
    private(set) var errorMessage: String?

    init() {
        refresh()
    }

    func refresh() {
        switch SMAppService.mainApp.status {
        case .enabled:
            isEnabled = true
            needsApproval = false
        case .requiresApproval:
            isEnabled = true
            needsApproval = true
        default:
            isEnabled = false
            needsApproval = false
        }
    }

    func setEnabled(_ enabled: Bool) {
        errorMessage = nil
        do {
            if enabled {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
        } catch {
            errorMessage = error.localizedDescription
        }
        refresh()
    }

    /// Opens the Login Items pane, where an approval is granted.
    func openLoginItemsSettings() {
        SMAppService.openSystemSettingsLoginItems()
    }
}
