import Cocoa
import SwiftUI

struct ProviderSettingsRow: Identifiable {
    let provider: ProviderView

    var id: String { provider.id }
}

@MainActor
final class ProviderSettingsModel: ObservableObject {
    @Published var providers: [ProviderView] = []
    @Published var selectedProviderID: String?
    @Published var status = "正在读取供应商…"
    @Published var isBusy = false
    @Published var banner: ProviderBannerState?
    @Published var codexInstallMode: ManagedCodexInstallMode?
    @Published var displayName = ""
    @Published var baseURL = ""
    @Published var websiteURL = ""
    @Published var imageGenerationPath = ""
    @Published var apiKey = ""
    @Published var quotaUsername = ""
    @Published var quotaWorkspaceID = ""
    @Published var quotaAuthCookie = ""
    @Published var auxiliaryModelUpstream = false
    @Published var baiduAuthBridge = BaiduAuthBridgeMode.disabled
    @Published var baiduCodeReport = false

    var selectedProvider: ProviderView? {
        providers.first { $0.id == selectedProviderID }
    }

    var canAddProvider: Bool { !isBusy }
    var canModifySelectedProvider: Bool {
        !isBusy && selectedProvider?.kind == .configured
    }

    func rows() -> [ProviderSettingsRow] {
        providers.map(ProviderSettingsRow.init(provider:))
    }

    func selectProvider(_ providerID: String?) {
        selectedProviderID = providerID
        guard let provider = selectedProvider else {
            status = providers.isEmpty ? "等待新增 Provider" : "请选择 Provider"
            return
        }
        displayName = provider.displayName
        baseURL = provider.baseURL
        websiteURL = provider.websiteURL ?? ""
        imageGenerationPath = provider.imageGenerationPath ?? ""
        apiKey = ""
        quotaUsername = provider.quotaUsername ?? ""
        quotaWorkspaceID = provider.quotaWorkspaceID ?? ""
        quotaAuthCookie = ""
        auxiliaryModelUpstream = provider.auxiliaryModelUpstream
        baiduAuthBridge = provider.effectiveBaiduAuthBridge ?? .disabled
        baiduCodeReport = provider.baiduCodeReport == true
        status = selectedProviderStatus(
            provider: provider,
            providersEmpty: providers.isEmpty,
            codexInstallMode: codexInstallMode
        )
    }
}

struct ProviderBannerState: Equatable {
    let text: String
    let isError: Bool
}

struct ProviderSettingsRootView: View {
    @ObservedObject var model: ProviderSettingsModel
    let onAdd: () -> Void
    let onRemove: () -> Void
    let onToggle: () -> Void
    let onTest: () -> Void
    let onSave: () -> Void
    let onClearKey: () -> Void
    let onClearQuotaCredentials: () -> Void
    let onMove: (IndexSet, Int) -> Void

    var body: some View {
        NavigationSplitView {
            VStack(spacing: 0) {
                HStack(spacing: 6) {
                    Text("供应商")
                        .font(.headline)
                    Spacer()

                    HStack(spacing: 0) {
                        Button(action: onAdd) {
                            Image(systemName: "plus")
                                .frame(width: 26, height: 22)
                        }
                        .help("新增 Provider")
                        .accessibilityLabel("新增 Provider")
                        .disabled(!model.canAddProvider)

                        Divider()
                            .frame(height: 14)

                        Button(action: onRemove) {
                            Image(systemName: "minus")
                                .frame(width: 26, height: 22)
                        }
                        .help("删除 Provider")
                        .accessibilityLabel("删除 Provider")
                        .disabled(!model.canModifySelectedProvider)
                    }
                    .buttonStyle(.borderless)
                    .controlSize(.small)
                    .foregroundStyle(.secondary)
                    .liquidGlass(in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 10)

                List(selection: Binding(
                    get: { model.selectedProviderID },
                    set: { model.selectProvider($0) }
                )) {
                    if model.rows().isEmpty {
                        Label("还没有 Provider", systemImage: "plus.circle")
                            .foregroundStyle(.secondary)
                            .padding(.vertical, 6)
                    } else {
                        ForEach(model.rows()) { row in
                            ProviderSidebarRow(provider: row.provider)
                                .tag(row.id)
                                .moveDisabled(row.provider.kind == .official)
                        }
                        .onMove(perform: onMove)
                    }
                }
                .listStyle(.sidebar)
                .scrollContentBackground(.hidden)
                .background(Color.clear)

                HStack(spacing: 6) {
                    Image(systemName: "arrow.up.arrow.down")
                        .font(.caption.weight(.medium))
                    Text(sidebarFooterText)
                        .font(.caption)
                        .lineLimit(1)
                }
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
                .background(.ultraThinMaterial)
            }
            .background(.regularMaterial)
            .clipShape(Rectangle())
            .navigationSplitViewColumnWidth(min: 300, ideal: 340, max: 400)
        } detail: {
            VStack(alignment: .leading, spacing: 0) {
                if let banner = model.banner {
                    HStack {
                        Label {
                            Text(banner.text)
                                .lineLimit(2)
                        } icon: {
                            Image(systemName: banner.isError ? "exclamationmark.triangle.fill" : "checkmark.circle.fill")
                        }
                        Spacer()
                    }
                    .font(.footnote.weight(.medium))
                    .foregroundStyle(banner.isError ? Color(nsColor: .systemRed) : Color(nsColor: .systemGreen))
                    .padding(.horizontal, 12)
                    .padding(.vertical, 9)
                    .background(
                        (banner.isError ? Color(nsColor: .systemRed) : Color(nsColor: .systemGreen)).opacity(0.10),
                        in: RoundedRectangle(cornerRadius: 10, style: .continuous)
                    )
                    .padding(.horizontal, 24)
                    .padding(.top, 14)
                }

                if let provider = model.selectedProvider {
                    ProviderDetailForm(
                        provider: provider,
                        codexInstallMode: model.codexInstallMode,
                        isBusy: model.isBusy,
                        formState: model,
                        onToggle: onToggle,
                        onTest: onTest,
                        onSave: onSave,
                        onClearKey: onClearKey,
                        onClearQuotaCredentials: onClearQuotaCredentials
                    )
                } else {
                    ProviderEmptyState(
                        isEmpty: model.providers.isEmpty,
                        onAdd: onAdd
                    )
                }
            }
            .liquidGlassWindowBackground()
        }
        .navigationTitle("供应商设置")
    }

    private var sidebarFooterText: String {
        let configuredCount = model.providers.count { $0.kind == .configured }
        return configuredCount == 0
            ? "新增自定义 Provider 后可排序"
            : "\(configuredCount) 个自定义 Provider 可拖动排序"
    }
}

private struct ProviderEmptyState: View {
    let isEmpty: Bool
    let onAdd: () -> Void

    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: isEmpty ? "server.rack" : "cursorarrow.click.2")
                .font(.system(size: 28, weight: .medium))
                .foregroundStyle(.tertiary)
            Text(isEmpty ? "还没有 Provider" : "选择一个 Provider")
                .font(.title2.weight(.semibold))
            Text(
                isEmpty
                    ? "添加上游 API 后，Codex Mixin 才能为网关提供模型。"
                    : "从左侧选择一个 Provider，查看连接状态和路由配置。"
            )
            .font(.body)
            .foregroundStyle(.secondary)
            .multilineTextAlignment(.center)
            if isEmpty {
                Button("新增 Provider", action: onAdd)
                    .buttonStyle(.borderedProminent)
                    .controlSize(.large)
                    .keyboardShortcut(.defaultAction)
            }
        }
        .frame(maxWidth: 440)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(32)
    }
}

private struct ProviderSidebarRow: View {
    let provider: ProviderView

    var body: some View {
        HStack(spacing: 10) {
            ProviderLogo(provider: provider, size: 28)

            VStack(alignment: .leading, spacing: 3) {
                Text(provider.displayName)
                    .font(.system(size: 13, weight: .semibold))
                    .lineLimit(1)
                HStack(spacing: 5) {
                    Circle()
                        .fill(providerStatusColor(provider))
                        .frame(width: 6, height: 6)
                    Text(sidebarMetadata)
                        .font(.caption)
                        .lineLimit(1)
                }
                .foregroundStyle(.secondary)
            }

            Spacer(minLength: 4)

            if provider.kind == .configured {
                Image(systemName: "line.3.horizontal")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.tertiary)
                    .help("拖动以排序")
                    .accessibilityLabel("可拖动排序")
            }
        }
        .padding(.vertical, 4)
    }

    private var sidebarMetadata: String {
        if provider.kind == .official {
            return "官方 · \(readinessLabel(provider.readiness))"
        }
        let modelCount = "\(provider.selectedModels.count)/\(provider.cachedModels.count) 个模型"
        return provider.enabled
            ? "\(readinessLabel(provider.readiness)) · \(modelCount)"
            : "已停用 · \(modelCount)"
    }
}

private struct ProviderDetailForm: View {
    let provider: ProviderView
    let codexInstallMode: ManagedCodexInstallMode?
    let isBusy: Bool
    @ObservedObject var formState: ProviderSettingsModel
    let onToggle: () -> Void
    let onTest: () -> Void
    let onSave: () -> Void
    let onClearKey: () -> Void
    let onClearQuotaCredentials: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            Form {
            ProviderDetailHeader(provider: provider)

            Section {
                LabeledContent("Provider ID") {
                    Text(provider.id)
                        .font(.body.monospaced())
                        .textSelection(.enabled)
                }
                if provider.kind == .official {
                    Label("此 Provider 由 Codex 官方 OAuth 登录管理，不能在这里修改连接参数。", systemImage: "lock.fill")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                } else {
                    if isCustom {
                        TextField("站点名称", text: $formState.displayName)
                        TextField("API 地址", text: $formState.baseURL)
                        TextField("官网地址", text: $formState.websiteURL)
                    }
                    TextField("绘图接口路径", text: $formState.imageGenerationPath, prompt: Text("/v1/images/generations"))
                    HStack(spacing: 8) {
                        SecureField(apiKeyPrompt, text: $formState.apiKey)
                        if apiKeyConfigured {
                            Button("清除密钥", action: onClearKey)
                                .disabled(isBusy || !provider.apiKeyConfigured)
                        }
                    }
                    if apiKeyConfigured {
                    }
                }
            } header: {
                Text("连接配置")
            }

            if isBaiduOneAPI {
                Section {
                    TextField("额度用户名", text: $formState.quotaUsername, prompt: Text("Baidu OneAPI 额度接口必填"))
                    Picker("认证桥接", selection: $formState.baiduAuthBridge) {
                        Text(AppLocalization.string("settings.disabledDefault")).tag(BaiduAuthBridgeMode.disabled)
                        Text("DUCX 核心（loopback）").tag(BaiduAuthBridgeMode.ducxLoopback)
                    }
                    Toggle("上报 AI 代码使用数据", isOn: $formState.baiduCodeReport)
                } header: {
                    Text("百度额度")
                }
            }

            if isOpenCodeGo {
                Section {
                    TextField("工作区 ID", text: $formState.quotaWorkspaceID, prompt: Text("例如：wrk_abc123"))
                    HStack(spacing: 8) {
                        SecureField("Auth Cookie", text: $formState.quotaAuthCookie, prompt: Text(authCookiePrompt))
                        if quotaCookieConfigured {
                            Button("清除额度凭据", action: onClearQuotaCredentials)
                                .disabled(isBusy || provider.quotaAuthCookieConfigured != true)
                        }
                    }
                } header: {
                    Text("OpenCode Go")
                }
            }

            Section {
                Toggle(
                    AppLocalization.string("providerSettings.useForVoiceAutoReviewAndOther"),
                    isOn: $formState.auxiliaryModelUpstream
                )
                .disabled(provider.kind == .official || !isAuxiliaryModelUpstreamSelectable(for: provider, codexInstallMode: codexInstallMode))
                .help(auxiliaryModelTooltip(for: provider, codexInstallMode: codexInstallMode))
            } header: {
                Text("辅助模型路由")
            }
            }
            .formStyle(.grouped)
            .scrollContentBackground(.hidden)
            .frame(maxWidth: 760)
            .frame(maxWidth: .infinity)

            ProviderActionBar(
                isBusy: isBusy,
                toggleTitle: toggleTitle,
                canModify: formState.canModifySelectedProvider,
                onToggle: onToggle,
                onTest: onTest,
                onSave: onSave
            )
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear(perform: loadProvider)
        .onChange(of: provider.id) { loadProvider() }
    }

    private var isCustom: Bool { provider.presetID == "custom" }
    private var isBaiduOneAPI: Bool { provider.presetID == "baidu-oneapi" }
    private var isOpenCodeGo: Bool { requiresOpenCodeGoQuotaCredentials(provider.presetID ?? "") }
    private var apiKeyConfigured: Bool { provider.apiKeyConfigured && !isBusy }
    private var quotaCookieConfigured: Bool { provider.quotaAuthCookieConfigured == true && !isBusy }

    private var toggleTitle: String {
        provider.enabled ? "停用" : "启用"
    }

    private var apiKeyPrompt: String {
        provider.apiKeyConfigured ? "已配置；留空保留" : "尚未配置；启用前必须填写"
    }

    private var authCookiePrompt: String {
        provider.quotaAuthCookieConfigured == true ? "已配置；留空保留" : "opencode.ai auth cookie"
    }

    private func loadProvider() {
        formState.displayName = provider.displayName
        formState.baseURL = provider.baseURL
        formState.websiteURL = provider.websiteURL ?? ""
        formState.imageGenerationPath = provider.imageGenerationPath ?? ""
        formState.apiKey = ""
        formState.quotaUsername = provider.quotaUsername ?? ""
        formState.quotaWorkspaceID = provider.quotaWorkspaceID ?? ""
        formState.quotaAuthCookie = ""
        formState.auxiliaryModelUpstream = provider.auxiliaryModelUpstream
        formState.baiduAuthBridge = provider.effectiveBaiduAuthBridge ?? .disabled
        formState.baiduCodeReport = provider.baiduCodeReport == true
    }
}

private struct ProviderActionBar: View {
    let isBusy: Bool
    let toggleTitle: String
    let canModify: Bool
    let onToggle: () -> Void
    let onTest: () -> Void
    let onSave: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            Divider()
            HStack(spacing: 8) {
                if isBusy {
                    ProgressView()
                        .controlSize(.small)
                }
                Spacer()
                Button(toggleTitle, action: onToggle)
                    .disabled(!canModify)
                Button("测试连接", action: onTest)
                    .disabled(!canModify)
                Button("保存更改", action: onSave)
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.glassProminent)
                    .disabled(!canModify)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 10)
            .background(.bar)
        }
    }
}

private struct ProviderDetailHeader: View {
    let provider: ProviderView

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            ProviderLogo(provider: provider, size: 42)

            VStack(alignment: .leading, spacing: 5) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(provider.displayName)
                        .font(.title3.weight(.semibold))
                        .lineLimit(1)
                    ProviderStateBadge(provider: provider)
                }
                Text(provider.kind == .official ? "官方 Provider · 只读" : provider.id)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                if !provider.readinessIssues.isEmpty {
                    Label(provider.readinessIssues.joined(separator: "；"), systemImage: "exclamationmark.triangle.fill")
                        .font(.caption)
                        .foregroundStyle(.orange)
                        .lineLimit(2)
                }
            }

            Spacer(minLength: 16)

            Text("\(provider.routableModelCount) 个可路由模型")
                .font(.callout.weight(.medium))
                .foregroundStyle(.secondary)
        }
        .padding(.vertical, 8)
    }
}

private struct ProviderLogo: View {
    let provider: ProviderView
    let size: CGFloat

    var body: some View {
        Group {
            if let image = providerLogoImageForSettings(provider) {
                Image(nsImage: image)
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .padding(size * 0.18)
            } else {
                Image(systemName: "server.rack")
                    .font(.system(size: size * 0.42, weight: .medium))
                    .foregroundStyle(.secondary)
            }
        }
        .frame(width: size, height: size)
        .background(.quaternary.opacity(0.45), in: RoundedRectangle(cornerRadius: size * 0.22, style: .continuous))
    }
}

private func providerLogoImageForSettings(_ provider: ProviderView) -> NSImage? {
    if let cached = cachedProviderLogoImage(providerID: provider.id, websiteURL: provider.websiteURL) {
        return cached
    }

    let normalized = (provider.presetID ?? provider.id).lowercased()
    let assetName: String
    if normalized.contains("baidu") {
        assetName = "baidu"
    } else if normalized.contains("deepseek") {
        assetName = "deepseek"
    } else if normalized.contains("opencode") {
        assetName = "opencode"
    } else if normalized.contains("openrouter") {
        assetName = "openrouter"
    } else if normalized.contains("openai") || normalized.contains("chatgpt") || provider.kind == .official {
        assetName = "openai"
    } else {
        assetName = "custom"
    }

    let directories = [
        Bundle.main.resourceURL?.appendingPathComponent("ProviderLogos", isDirectory: true),
        Bundle.main.bundleURL.appendingPathComponent("ProviderLogos", isDirectory: true),
    ]
    for directory in directories.compactMap({ $0 }) {
        if let image = NSImage(contentsOf: directory.appendingPathComponent("\(assetName).svg")) {
            return image
        }
    }
    return nil
}

private func providerStatusColor(_ provider: ProviderView) -> Color {
    guard provider.kind == .official || provider.enabled else {
        return .secondary
    }
    switch provider.readiness {
    case "healthy": return .green
    case "degraded": return .orange
    default: return .secondary
    }
}

private struct ProviderStateBadge: View {
    let provider: ProviderView

    private var label: String {
        if provider.kind == .official {
            return readinessLabel(provider.readiness)
        }
        return provider.enabled ? readinessLabel(provider.readiness) : "已停用"
    }

    private var color: Color {
        if !provider.enabled && provider.kind != .official {
            return .secondary
        }
        switch provider.readiness {
        case "healthy": return .green
        case "degraded": return .orange
        case "disabled": return .secondary
        default: return .secondary
        }
    }

    var body: some View {
        Text(label)
            .font(.caption.weight(.medium))
            .foregroundStyle(color)
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(color.opacity(0.12), in: Capsule())
    }
}
