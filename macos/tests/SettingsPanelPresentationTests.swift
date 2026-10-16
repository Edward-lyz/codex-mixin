import Cocoa

func menuItemImage(_ systemSymbolName: String) -> NSImage? {
    nil
}

func showAlert(title: String, message: String) {
    preconditionFailure("The presentation test must not display an alert")
}

@main
struct SettingsPanelPresentationTests {
    static func main() {
        _ = NSApplication.shared
        let catalogPath = ProcessInfo.processInfo.environment["CODEX_MIXIN_TEST_INTERFACE_JSON"]!
        let interface = try! String(contentsOfFile: catalogPath, encoding: .utf8)
        let catalog = try! ProviderCatalog.decodeInterface(interface)
        let parent = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 800, height: 600),
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
        var resultWasDelivered = false

        runAddProviderSheet(attachedTo: parent, catalog: catalog) { values in
            precondition(values == nil)
            resultWasDelivered = true
        }

        guard let sheet = parent.attachedSheet else {
            preconditionFailure("The add-provider form must be attached to its settings window")
        }
        precondition(!(sheet is NSPanel), "The add-provider form must be a regular NSWindow")
        precondition(sheet.level == .normal, "The add-provider sheet must use the normal window level")
        precondition(NSApp.modalWindow == nil, "The add-provider form must not start an app-modal loop")
        precondition(sheet.contentViewController != nil, "The sheet must host its SwiftUI form")

        let model = AddProviderFormModel(catalog: catalog)
        precondition(model.presets.map(\.id) == catalog.presets.map(\.id))
        precondition(
            model.presets.contains { $0.id == "agy" },
            "The add-provider picker must include the Antigravity preset"
        )
        precondition(
            model.isBaiduOneAPI && !model.isCustom && !model.requiresQuotaCredentials,
            "Baidu fields must be active for the default preset"
        )
        model.preset = "opencode-go"
        precondition(model.requiresQuotaCredentials, "OpenCode Go must require quota credentials")
        model.preset = "aws-bedrock"
        precondition(
            model.isAWSBedrock && model.credentialURL != nil && model.awsRegion == "us-east-1",
            "Amazon Bedrock must expose AWS AK/SK fields and setup documentation"
        )
        model.preset = "custom"
        precondition(model.isCustom && model.credentialURL == nil, "Custom providers must show URL fields")
        model.preset = "agy"
        precondition(
            model.validatedValues()?.apiKey == "",
            "Antigravity must allow the CLI to load local OAuth credentials without an API key"
        )
        // The OAuth form hides the key field. A key left over from another
        // preset must not reach the CLI, because a non-empty key makes
        // `providers add --preset agy` skip browser login.
        model.apiKey = " stale-key "
        precondition(
            model.validatedValues()?.apiKey == "",
            "Antigravity form must not submit a hidden API key"
        )
        var document = try! JSONSerialization.jsonObject(with: Data(interface.utf8)) as! [String: Any]
        var rawCatalog = document["provider_catalog"] as! [String: Any]
        var presets = rawCatalog["presets"] as! [[String: Any]]
        var future = presets.first { $0["id"] as? String == "agy" }!
        future["id"] = "future-provider"
        future["display_name"] = "Future Provider"
        presets.append(future)
        rawCatalog["presets"] = presets
        rawCatalog["default_preset"] = "future-provider"
        document["provider_catalog"] = rawCatalog
        let futureJSON = String(decoding: try! JSONSerialization.data(withJSONObject: document), as: UTF8.self)
        let futureCatalog = try! ProviderCatalog.decodeInterface(futureJSON)
        let futureModel = AddProviderFormModel(catalog: futureCatalog)
        precondition(futureModel.preset == "future-provider" && futureModel.usesLocalOAuth)
        precondition(futureModel.validatedValues()?.preset == "future-provider")
        precondition(futureModel.presets.count == model.presets.count + 1,
                     "New CLI presets must appear without updating the Swift preset list")
        do {
            _ = try ProviderCatalog.decodeInterface("{\"protocol_version\":1}")
            preconditionFailure("Missing CLI metadata must fail explicitly")
        } catch {}

        parent.endSheet(sheet, returnCode: .cancel)
        RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.1))
        precondition(resultWasDelivered)
        print("Add-provider sheet presentation: passed")
    }

}
