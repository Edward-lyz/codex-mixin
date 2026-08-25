import Cocoa
import SwiftUI

enum CodexInstallMode: Equatable {
    case openAIAccount
    case customModelsOnly

    var commandArguments: [String] {
        switch self {
        case .openAIAccount:
            return ["install-codex", "--codex-oauth-proxy"]
        case .customModelsOnly:
            return ["install-codex", "--custom-only"]
        }
    }

    var completionMessage: String {
        switch self {
        case .openAIAccount:
            return "官方账号模式已安装：官方 GPT、插件、云任务和账户功能保持可用，自定义模型同时加入模型选择器。请重启 Codex App；CLI 需要开新会话。"
        case .customModelsOnly:
            return "仅自定义模型模式已安装：模型选择器会显示自定义模型；界面中的 amazon-bedrock 只是本地登录占位，不会连接 AWS。官方插件、云任务和账户功能不可用。以后登录官方账号时，请先“从 Codex 恢复”，再改装官方账号模式。"
        }
    }

    var providerDescription: String {
        switch self {
        case .openAIAccount: return "codex-mixin / 官方 OAuth 代理"
        case .customModelsOnly: return "amazon-bedrock / 本地占位（不连接 AWS）"
        }
    }
}

private struct InstallCodexView: View {
    @State private var selectedMode: CodexInstallMode?
    let cancel: () -> Void
    let install: (CodexInstallMode) -> Void

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section {
                    Picker("安装模式", selection: $selectedMode) {
                        Text("官方账号模式（推荐：已登录 Codex）")
                            .tag(Optional(CodexInstallMode.openAIAccount))
                        Text("仅自定义模型模式（没有官方登录也可用）")
                            .tag(Optional(CodexInstallMode.customModelsOnly))
                    }
                    .pickerStyle(.radioGroup)
                } header: {
                    Text("选择 Codex 安装模式")
                } footer: {
                    Text("安装会备份 ~/.codex/config.toml；仅自定义模式还会备份 auth.json。切换模式前，请先执行从 Codex 恢复。")
                }

                Section("模式说明") {
                    InstallModeDescription(
                        title: "官方账号模式",
                        systemImage: "person.crop.circle.badge.checkmark",
                        detail: "前提是已在官方 Codex App 登录并打开过一次。保留官方认证、GPT、插件、云任务和账户功能；自定义模型经本地网关加入同一个模型选择器。",
                        isActive: selectedMode == .openAIAccount
                    )
                    InstallModeDescription(
                        title: "仅自定义模型模式",
                        systemImage: "network",
                        detail: "用本地 Bedrock 形态的占位身份开启模型选择器。请求只到本地网关，不连接 AWS；官方插件、云任务和账户功能不可用。",
                        isActive: selectedMode == .customModelsOnly
                    )
                }

                Section("安装位置") {
                    LabeledContent("Codex 配置") {
                        Text("~/.codex/config.toml").textSelection(.enabled)
                    }
                    LabeledContent("模型目录") {
                        Text("~/.codex/model-catalogs/mixin-models.json").textSelection(.enabled)
                    }
                    LabeledContent("Provider") {
                        Text(selectedMode?.providerDescription ?? "选择模式后显示")
                            .foregroundStyle(selectedMode == nil ? .secondary : .primary)
                            .textSelection(.enabled)
                    }
                }
            }
            .formStyle(.grouped)
            .scrollContentBackground(.hidden)

            Divider()
            HStack {
                Spacer()
                Button("取消", action: cancel)
                    .keyboardShortcut(.cancelAction)
                Button("安装") {
                    if let selectedMode {
                        install(selectedMode)
                    }
                }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.borderedProminent)
                .disabled(selectedMode == nil)
            }
            .padding(20)
        }
        .frame(minWidth: 760, minHeight: 510)
        .background(Color(nsColor: .windowBackgroundColor))
    }
}

private struct InstallModeDescription: View {
    let title: String
    let systemImage: String
    let detail: String
    let isActive: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: systemImage)
                .font(.title2)
                .foregroundStyle(isActive ? Color.accentColor : Color.secondary)
                .frame(width: 28)
            VStack(alignment: .leading, spacing: 4) {
                Text(title).font(.headline)
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.vertical, 4)
        .opacity(isActive ? 1 : 0.72)
    }
}

@MainActor
func runInstallCodexPanel() -> CodexInstallMode? {
    let panel = NSWindow(
        contentRect: NSRect(x: 0, y: 0, width: 820, height: 560),
        styleMask: [.titled, .closable],
        backing: .buffered,
        defer: false
    )
    panel.title = "安装到 Codex"
    panel.level = .normal
    panel.center()
    configurePersistentWindow(panel)

    var confirmedMode: CodexInstallMode?
    let cancel = { NSApp.stopModal(withCode: .cancel) }
    let install = { mode in
        confirmedMode = mode
        NSApp.stopModal(withCode: .OK)
    }
    panel.contentViewController = NSHostingController(
        rootView: InstallCodexView(cancel: cancel, install: install)
    )

    let closeTarget = ModalActionTarget(cancel)
    panel.standardWindowButton(.closeButton)?.target = closeTarget
    panel.standardWindowButton(.closeButton)?.action = #selector(ModalActionTarget.run(_:))

    presentPersistentWindow(panel)
    let response = NSApp.runModal(for: panel)
    panel.close()
    _ = closeTarget
    return response == .OK ? confirmedMode : nil
}
