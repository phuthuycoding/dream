import SwiftUI
import SwiftTerm

/// SwiftUI bridge onto SwiftTerm's iOS `TerminalView` — a UIScrollView,
/// so touch scroll with inertia, scrollback freeze while the user reads
/// back, and bottom auto-follow are all native to the view. The
/// `TerminalAccessory` soft-key bar (esc/ctrl/tab/arrows) is installed
/// by the view itself. The controller is the delegate: keystrokes become
/// `i` frames, geometry changes become `r` frames.
struct TerminalView: UIViewRepresentable {
    let controller: TerminalController

    func makeUIView(context: Context) -> MetalTerminalView {
        let view = MetalTerminalView(frame: .zero)
        view.terminalDelegate = controller
        view.accessibilityIdentifier = "terminal.view"
        controller.bind(view)
        return view
    }

    func updateUIView(_ uiView: MetalTerminalView, context: Context) {}
}

/// Switches to the Metal renderer once the view is in a window — the
/// pipeline builds on a live MTKView. A failure stays on the
/// CoreGraphics path rather than leaving a blank terminal.
final class MetalTerminalView: SwiftTerm.TerminalView {
    private var metalAttempted = false

    override func didMoveToWindow() {
        super.didMoveToWindow()
        guard window != nil, !metalAttempted else { return }
        metalAttempted = true
        do {
            try setUseMetal(true)
        } catch {
            NSLog("Dream terminal: Metal renderer unavailable, keeping CoreGraphics: \(error)")
        }
        _ = becomeFirstResponder()
    }
}
