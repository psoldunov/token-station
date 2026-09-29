import AppKit
import SwiftUI

/// The panel on a backdrop, so a render shows its rounded edge.
///
/// The app itself draws no background: a window-style menu bar extra is given
/// the system's own material. Off screen a material has no desktop to sample and
/// comes out flat grey, so the render substitutes the opaque window colour.
struct RenderScene<Content: View>: View {
    var content: Content

    var body: some View {
        content
            .background(Color(nsColor: .windowBackgroundColor))
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .strokeBorder(Color(nsColor: .separatorColor), lineWidth: 1)
            }
            .padding(14)
            .background(Color(nsColor: .underPageBackgroundColor))
    }
}
