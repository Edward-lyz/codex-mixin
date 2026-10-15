import Cocoa
import SwiftUI

struct AddProviderFormValues {
    let definition: AddProviderPreset
    var preset: String { definition.id }
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
}

enum ProviderCredentialMode: String, Decodable {
    case apiKey = "api_key"
    case localOAuth = "local_oauth"
    case awsSigV4 = "aws_sigv4"
}

struct AddProviderPreset: Identifiable, Decodable {
    let id: String
    let displayName: String
    let authMode: ProviderCredentialMode
    let apiKeyRequired: Bool
    let credentialUrl: String?
    let customEndpoint: Bool
    let quotaUsernameRequired: Bool
    let quotaCredentialsRequired: Bool
}

struct ProviderProtocolOption: Identifiable, Decodable {
    let id: String
    let displayName: String
    let apiPath: String
}

struct ProviderCatalog: Decodable {
    let defaultPreset: String
    let defaultProtocol: String
    let defaultAwsRegion: String
    let presets: [AddProviderPreset]
    let protocols: [ProviderProtocolOption]

    static func decodeInterface(_ json: String) throws -> ProviderCatalog {
        struct Interface: Decodable {
            let protocolVersion: Int
            let providerCatalog: ProviderCatalog
        }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let interface = try decoder.decode(Interface.self, from: Data(json.utf8))
        let catalog = interface.providerCatalog
        guard interface.protocolVersion == 1,
              catalog.presets.contains(where: { $0.id == catalog.defaultPreset }),
              catalog.protocols.contains(where: { $0.id == catalog.defaultProtocol }) else {
            throw DecodingError.dataCorrupted(.init(
                codingPath: [], debugDescription: "Invalid CLI provider catalog"
            ))
        }
        return catalog
    }
}

@MainActor
final class AddProviderFormModel: ObservableObject {
    let presets: [AddProviderPreset]
    @Published var preset: String
    @Published var displayName = ""
    @Published var baseURL = ""
    @Published var websiteURL = ""
    @Published var apiKey = ""
    @Published var awsAccessKeyID = ""
    @Published var awsSecretAccessKey = ""
    @Published var awsSessionToken = ""
    @Published var awsRegion: String
    @Published var quotaUsername = ""
    @Published var quotaWorkspaceID = ""
    @Published var quotaAuthCookie = ""

    init(catalog: ProviderCatalog) {
        presets = catalog.presets
        preset = catalog.defaultPreset
        awsRegion = catalog.defaultAwsRegion
    }

    private var definition: AddProviderPreset? { presets.first { $0.id == preset } }
    var isCustom: Bool { definition?.customEndpoint == true }
    var isBaiduOneAPI: Bool { definition?.quotaUsernameRequired == true }
    var isAWSBedrock: Bool { definition?.authMode == .awsSigV4 }
    var usesLocalOAuth: Bool { definition?.authMode == .localOAuth }
    var requiresQuotaCredentials: Bool { definition?.quotaCredentialsRequired == true }
    var credentialURL: URL? { definition?.credentialUrl.flatMap(URL.init(string:)) }

    func validatedValues() -> AddProviderFormValues? {
        guard let definition else { return nil }
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
        guard !definition.apiKeyRequired || !trimmedAPIKey.isEmpty else {
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
            definition: definition,
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
            quotaAuthCookie: trimmedAuthCookie
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
                            Text(preset.displayName).tag(preset.id)
                        }
                    }

                    if let credentialURL = model.credentialURL, !model.isCustom {
                        Link(destination: credentialURL) {
                            Label(AppLocalization.string("settings.openProviderSetup"), systemImage: "key")
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
                            prompt: Text(AppLocalization.string("settings.apiURLPrompt"))
                        )
                        Text(AppLocalization.string("settings.apiURLHint"))
                            .font(.caption)
                            .foregroundStyle(.secondary)
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
                    if model.usesLocalOAuth {
                        Text(AppLocalization.string("settings.localOAuthCredentials"))
                            .font(.callout)
                            .foregroundStyle(.secondary)
                    }
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
                            prompt: Text(AppLocalization.string(model.usesLocalOAuth
                                ? "settings.optionalAPIKey" : "settings.requiredStoredOnlyByTheLocalRust"))
                        )
                    }

                    if model.isBaiduOneAPI {
                        TextField(
                            AppLocalization.string("settings.quotaUsername"),
                            text: $model.quotaUsername,
                            prompt: Text(AppLocalization.string("settings.baiduOneAPIQuotaUsername"))
                        )
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
                    .liquidGlassProminentButton()
            }
            .padding(20)
            .background(Color(nsColor: .windowBackgroundColor))
            .overlay(Rectangle().stroke(.separator))
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
    catalog: ProviderCatalog,
    completion: @escaping (AddProviderFormValues?) -> Void
) {
    let model = AddProviderFormModel(catalog: catalog)
    let sheet = NSWindow(
        contentRect: NSRect(x: 0, y: 0, width: 650, height: 560),
        styleMask: [.titled, .closable],
        backing: .buffered,
        defer: false
    )
    sheet.title = AppLocalization.string("settings.addProvider")
    configureOpaqueWindow(sheet)
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
