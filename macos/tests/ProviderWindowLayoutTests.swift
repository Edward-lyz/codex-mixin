import Cocoa
import SwiftUI

@main
struct ProviderWindowLayoutTests {
    @MainActor
    static func main() throws {
        _ = NSApplication.shared

        let largeScreen = NSRect(x: 0, y: 0, width: 1440, height: 900)
        let largeSize = providerSettingsContentSize(for: largeScreen)
        precondition(largeSize.width == 1_180)
        precondition(largeSize.height == 720)
        precondition(largeSize.width <= largeScreen.width - 48)
        precondition(largeSize.height <= largeScreen.height - 48)

        let smallScreen = NSRect(x: 0, y: 0, width: 1_024, height: 700)
        let smallSize = providerSettingsContentSize(for: smallScreen)
        precondition(smallSize.width <= smallScreen.width - 32)
        precondition(smallSize.height <= smallScreen.height - 32)
        precondition(providerSidebarMinimumWidth == 220)
        precondition(providerSidebarIdealWidth == 250)
        precondition(providerSidebarMaximumWidth == 300)
        precondition(providerSidebarMinimumWidth < providerSidebarIdealWidth)
        precondition(providerSidebarIdealWidth < providerSidebarMaximumWidth)
        precondition(modelTableMinimumWidth(includesRatio: false) <= 800)
        precondition(modelTableMinimumWidth(includesRatio: true) <= 800)
        precondition(
            modelTableMinimumWidth(includesRatio: true)
                > modelTableMinimumWidth(includesRatio: false)
        )

        try verifyModelColumns()
        print("Models and services window layout: passed")
    }

    @MainActor
    private static func verifyModelColumns() throws {
        for preset in ["baidu-oneapi", "custom"] {
            for title in ["Model", String(repeating: "Long model introduction ", count: 40)] {
                let response = try decodeProviderList("""
                {
                  "config_version": 2,
                  "gateway_auth_configured": false,
                  "providers": [{
                    "id": "provider", "preset_id": "\(preset)",
                    "display_name": "Provider", "enabled": true,
                    "auxiliary_model_upstream": false,
                    "protocol": "open_ai_responses", "base_url": "https://example.com",
                    "api_path": "/v1/responses", "model_source": {"kind": "static"},
                    "api_key_configured": true, "quota_parser": "generic",
                    "selected_models": ["model"], "new_models": [],
                    "unavailable_selected_models": [],
                    "cached_models": [{"id": "model", "display_name": "\(title)",
                      "description": "\(title)", "context_window": 128000}],
                    "readiness": "healthy", "readiness_issues": [],
                    "routable_model_count": 1
                  }]
                }
                """)
                let model = ModelBenchmarkModel(
                    startHandler: { _, _, _ in preconditionFailure("Unexpected benchmark") },
                    fetchHandler: { nil },
                    loadProvidersHandler: { response },
                    saveSelectionsHandler: { _, _, _, _ in }
                )
                model.applyProviderList(response, selecting: "provider")
                let window = NSWindow(
                    contentRect: NSRect(x: 0, y: 0, width: 890, height: 400),
                    styleMask: [.titled, .resizable], backing: .buffered, defer: false
                )
                window.isReleasedWhenClosed = false
                let host = NSHostingView(rootView: ModelBenchmarkRootView(model: model, embedded: true))
                window.contentView = host
                window.orderFront(nil)
                defer { window.close() }
                var initialModelWidth: CGFloat = 0
                // Exercise the real SwiftUI Table at the initial width and after resizing.
                for width: CGFloat in [890, 800, 1100, 890] {
                    window.setContentSize(NSSize(width: width, height: 400))
                    RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.1))
                    host.layoutSubtreeIfNeeded()
                    guard let table = findTable(in: host) else {
                        preconditionFailure("Model table did not render")
                    }
                    precondition(table.numberOfRows == 1)
                    precondition(table.numberOfColumns == (preset == "baidu-oneapi" ? 7 : 6))
                    let lastColumn = host.convert(table.rect(ofColumn: table.numberOfColumns - 1), from: table)
                    precondition(lastColumn.maxX <= host.bounds.maxX + 1,
                        "Capabilities hidden at width \(width): \(lastColumn.maxX)")
                    let modelWidth = table.rect(ofColumn: 1).width
                    if initialModelWidth == 0 { initialModelWidth = modelWidth }
                    if width == 1100 {
                        precondition(modelWidth > initialModelWidth,
                            "Model column did not expand with the window")
                    }
                }
            }
        }
    }

    private static func findTable(in view: NSView) -> NSTableView? {
        if let table = view as? NSTableView { return table }
        return view.subviews.lazy.compactMap { findTable(in: $0) }.first
    }
}
