import Cocoa
import SwiftUI

struct AddProviderFormValues {
    let preset: String
    let displayName: String
    let baseURL: String
    let websiteURL: String
    let apiKey: String
    let awsAccessKeyID: String
    let awsSecretAccessKey: String
    let awsSessionToken: String
    let awsRegion: String
    let quotaUsername: String
    let quotaWorkspaceID: String
    let quotaAuthCookie: String
    let baiduAuthBridge: String
}

struct AddProviderPreset: Identifiable {
    let id: String
    let title: String
}

@MainActor
final class AddProviderFormModel: ObservableObject {
    let presets = [
        AddProviderPreset(id: "baidu-oneapi", title: "Baidu OneAPI"),
        AddProviderPreset(id: "openrouter", title: "OpenRouter"),
        AddProviderPreset(id: "deepseek", title: "DeepSeek"),
        AddProviderPreset(id: "opencode-go", title: "OpenCode Go"),
        AddProviderPreset(id: "aws-bedrock", title: "Amazon Bedrock (AK/SK)"),
        AddProviderPreset(id: "custom", title: AppLocalization.string("settings.customSite")),
    ]

    @Published var preset = "baidu-oneapi"
    @Published var displayName = ""
    @Published var baseURL = ""
    @Published var websiteURL = ""
    @Published var apiKey = ""
    @Published var awsAccessKeyID = ""
    @Published var awsSecretAccessKey = ""
    @Published var awsSessionToken = ""
    @Published var awsRegion = "us-east-1"
    @Published var quotaUsername = ""
    @Published var quotaWorkspaceID = ""
    @Published var quotaAuthCookie = ""
    @Published var baiduAuthBridge = "disabled"

    var isCustom: Bool { preset == "custom" }
    var isBaiduOneAPI: Bool { preset == "baidu-oneapi" }
    var isAWSBedrock: Bool { preset == "aws-bedrock" }
    var requiresQuotaCredentials: Bool { requiresOpenCodeGoQuotaCredentials(preset) }
    var credentialURL: URL? { URL(string: providerCredentialURL(preset)) }

    func validatedValues() -> AddProviderFormValues? {
        let trimmedDisplayName = displayName.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedBaseURL = baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
        if isCustom, trimmedDisplayName.isEmpty || trimmedBaseURL.isEmpty {
            showAlert(
                title: AppLocalization.string("settings.customSiteInformationRequired"),
                message: AppLocalization.string("settings.enterTheSiteNameAndAPIURL")
            )
            return nil
        }

        let trimmedAPIKey = apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard isAWSBedrock || !trimmedAPIKey.isEmpty else {
            showAlert(
                title: "缺少 API 密钥",
                message: AppLocalization.string("settings.enterTheProviderAPIKey")
            )
            return nil
        }

        let trimmedAccessKeyID = awsAccessKeyID.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedSecretAccessKey = awsSecretAccessKey.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedSessionToken = awsSessionToken.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedRegion = awsRegion.trimmingCharacters(in: .whitespacesAndNewlines)
        if isAWSBedrock,
           trimmedAccessKeyID.isEmpty || trimmedSecretAccessKey.isEmpty || trimmedRegion.isEmpty {
            showAlert(
                title: "缺少 AWS 凭据",
                message: "Amazon Bedrock 必须填写 Region、Access Key ID 和 Secret Access Key。"
            )
            return nil
        }

        let trimmedUsername = quotaUsername.trimmingCharacters(in: .whitespacesAndNewlines)
        if isBaiduOneAPI, trimmedUsername.isEmpty {
            showAlert(
                title: "缺少额度用户名",
                message: AppLocalization.string("settings.enterTheBaiduOneAPIQuotaUsername")
            )
            return nil
        }

        let trimmedWorkspaceID = quotaWorkspaceID.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedAuthCookie = quotaAuthCookie.trimmingCharacters(in: .whitespacesAndNewlines)
        if requiresQuotaCredentials, trimmedWorkspaceID.isEmpty || trimmedAuthCookie.isEmpty {
            showAlert(
                title: AppLocalization.string("settings.opencodeGoQuotaCredentialsRequired"),
                message: AppLocalization.string("settings.opencodeGoRequiresBothTheWorkspaceID")
            )
            return nil
        }

        return AddProviderFormValues(
            preset: preset,
            displayName: trimmedDisplayName,
            baseURL: trimmedBaseURL,
            websiteURL: websiteURL.trimmingCharacters(in: .whitespacesAndNewlines),
            apiKey: trimmedAPIKey,
            awsAccessKeyID: trimmedAccessKeyID,
            awsSecretAccessKey: trimmedSecretAccessKey,
            awsSessionToken: trimmedSessionToken,
            awsRegion: trimmedRegion,
            quotaUsername: trimmedUsername,
            quotaWorkspaceID: trimmedWorkspaceID,
            quotaAuthCookie: trimmedAuthCookie,
            baiduAuthBridge: baiduAuthBridge
        )
    }
}

private struct AddProviderFormView: View {
    @ObservedObject var model: AddProviderFormModel
    let cancel: () -> Void
    let submit: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section {
                    Picker(AppLocalization.string("settings.provider"), selection: $model.preset) {
                        ForEach(model.presets) { preset in
                            Text(preset.title).tag(preset.id)
                        }
                    }

                    if let credentialURL = model.credentialURL, !model.isCustom {
                        Link(destination: credentialURL) {
                            Label(AppLocalization.string("settings.openAPIKeyPage"), systemImage: "key")
                        }
                    }
                } header: {
                    Text(AppLocalization.string("settings.chooseASubscriptionAndEnterItsCredentials"))
                }

                if model.isCustom {
                    Section(AppLocalization.string("settings.customSite")) {
                        TextField(
                            AppLocalization.string("settings.siteName"),
                            text: $model.displayName,
                            prompt: Text(AppLocalization.string("settings.forExampleCommunityAPI"))
                        )
                        TextField(
                            AppLocalization.string("settings.apiURL"),
                            text: $model.baseURL,
                            prompt: Text("https://example.com/v1")
                        )
                        TextField("官网地址", text: $model.websiteURL, prompt: Text("https://example.com"))
                    }
                }

                if model.isAWSBedrock {
                    Section("Amazon Bedrock") {
                        TextField("AWS Region", text: $model.awsRegion, prompt: Text("us-east-1"))
                        Text("Endpoint 会按 Region 自动设置为 Bedrock Mantle。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }

                Section("凭据") {
                    if model.isAWSBedrock {
                        SecureField("Access Key ID", text: $model.awsAccessKeyID)
                        SecureField("Secret Access Key", text: $model.awsSecretAccessKey)
                        SecureField(
                            "Session Token（可选）",
                            text: $model.awsSessionToken
                        )
                    } else {
                        SecureField(
                            "API Key",
                            text: $model.apiKey,
                            prompt: Text(AppLocalization.string("settings.requiredStoredOnlyByTheLocalRust"))
                        )
                    }

                    if model.isBaiduOneAPI {
                        TextField(
                            AppLocalization.string("settings.quotaUsername"),
                            text: $model.quotaUsername,
                            prompt: Text(AppLocalization.string("settings.baiduOneAPIQuotaUsername"))
                        )
                        Picker(AppLocalization.string("settings.authBridge"), selection: $model.baiduAuthBridge) {
                            Text(AppLocalization.string("settings.disabledDefault")).tag("disabled")
                            Text("DUCX 核心（loopback）").tag("ducx_loopback")
                        }
                        .help(AppLocalization.string("settings.ducxUsesACodexMixinManagedCopy"))
                    }

                    if model.requiresQuotaCredentials {
                        TextField(
                            AppLocalization.string("settings.workspaceID"),
                            text: $model.quotaWorkspaceID,
                            prompt: Text(AppLocalization.string("settings.forExampleWrkAbc123"))
                        )
                        SecureField(
                            AppLocalization.string("settings.authCookie"),
                            text: $model.quotaAuthCookie,
                            prompt: Text(AppLocalization.string("settings.opencodeAiAuthCookie"))
                        )
                    }
                }
            }
            .formStyle(.grouped)
            .scrollContentBackground(.hidden)

            Divider()

            HStack {
                Spacer()
                Button(AppLocalization.string("settings.cancel"), action: cancel)
                    .keyboardShortcut(.cancelAction)
                Button(AppLocalization.string("settings.add"), action: submit)
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.borderedProminent)
            }
            .padding(20)
        }
        .frame(minWidth: 620, minHeight: 500)
        .background(Color(nsColor: .windowBackgroundColor))
    }
}

final class ModalActionTarget: NSObject {
    let action: () -> Void

    init(_ action: @escaping () -> Void) {
        self.action = action
    }

    @objc func run(_ sender: Any?) {
        action()
    }
}

@MainActor
func runAddProviderSheet(
    attachedTo parentWindow: NSWindow,
    completion: @escaping (AddProviderFormValues?) -> Void
) {
    let model = AddProviderFormModel()
    let sheet = NSWindow(
        contentRect: NSRect(x: 0, y: 0, width: 650, height: 560),
        styleMask: [.titled, .closable],
        backing: .buffered,
        defer: false
    )
    sheet.title = AppLocalization.string("settings.addProvider")
    configurePersistentWindow(sheet)

    var submittedValues: AddProviderFormValues?
    let cancel = {
        parentWindow.endSheet(sheet, returnCode: .cancel)
    }
    let submit = {
        guard let values = model.validatedValues() else { return }
        submittedValues = values
        parentWindow.endSheet(sheet, returnCode: .OK)
    }
    sheet.contentViewController = NSHostingController(
        rootView: AddProviderFormView(model: model, cancel: cancel, submit: submit)
    )

    let closeTarget = ModalActionTarget(cancel)
    sheet.standardWindowButton(.closeButton)?.target = closeTarget
    sheet.standardWindowButton(.closeButton)?.action = #selector(ModalActionTarget.run(_:))

    presentPersistentWindow(parentWindow)
    parentWindow.beginSheet(sheet) { response in
        sheet.close()
        _ = closeTarget
        completion(response == .OK ? submittedValues : nil)
    }
}

func requiresOpenCodeGoQuotaCredentials(_ provider: String) -> Bool {
    provider == "opencode-go"
}

func providerCredentialURL(_ provider: String) -> String {
    switch provider {
    case "baidu-oneapi": return "https://oneapi-comate.baidu-int.com/token"
    case "openrouter": return "https://openrouter.ai/settings/keys"
    case "deepseek": return "https://platform.deepseek.com/api_keys"
    case "opencode-go": return "https://opencode.ai/go"
    case "aws-bedrock": return "https://code.claude.com/docs/zh-CN/amazon-bedrock#2-configure-aws-credentials"
    default: return ""
    }
}
