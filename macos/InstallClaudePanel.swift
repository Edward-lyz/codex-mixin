import Cocoa
import SwiftUI

struct ClaudeModelOption: Identifiable, Hashable {
    let id: String
    let displayName: String
}

struct ClaudeModelMapping: Equatable {
    let opus: String
    let sonnet: String
    let haiku: String

    var commandArguments: [String] {
        [
            "install-claude",
            "--opus-model", opus,
            "--sonnet-model", sonnet,
            "--haiku-model", haiku,
        ]
    }
}

enum ClaudeInstallPanelError: Error, CustomStringConvertible {
    case invalidProviderJSON
    case noModels

    var description: String {
        switch self {
        case .invalidProviderJSON:
            return "模型列表不是有效的 Provider JSON"
        case .noModels:
            return "没有已启用、已选且可路由的模型"
        }
    }
}

private struct ClaudeProviderDocument: Decodable {
    let providers: [ClaudeProviderRecord]
}

private struct ClaudeProviderRecord: Decodable {
    let id: String?
    let displayName: String?
    let enabled: Bool?
    let selectedModels: [String]?
    let cachedModels: [ClaudeProviderModelRecord]?

    enum CodingKeys: String, CodingKey {
        case id
        case displayName = "display_name"
        case enabled
        case selectedModels = "selected_models"
        case cachedModels = "cached_models"
    }
}

private struct ClaudeProviderModelRecord: Decodable {
    let id: String
}

func decodeClaudeModelOptions(_ rawJSON: String) throws -> [ClaudeModelOption] {
    guard
        let document = try? JSONDecoder().decode(
            ClaudeProviderDocument.self,
            from: Data(rawJSON.utf8)
        )
    else {
        throw ClaudeInstallPanelError.invalidProviderJSON
    }

    return document.providers.flatMap { provider -> [ClaudeModelOption] in
        guard
            provider.enabled == true,
            let providerID = provider.id,
            let selectedModels = provider.selectedModels,
            let cachedModels = provider.cachedModels
        else {
            return []
        }
        let selected = Set(selectedModels)
        return cachedModels.compactMap { model in
            guard selected.contains(model.id) else {
                return nil
            }
            let providerName = provider.displayName ?? providerID
            return ClaudeModelOption(
                id: "\(model.id)-\(providerID)",
                displayName: "\(model.id) · \(providerName)"
            )
        }
    }.sorted {
        $0.displayName.localizedStandardCompare($1.displayName) == .orderedAscending
    }
}

func suggestedClaudeModelMapping(
    options: [ClaudeModelOption]
) throws -> ClaudeModelMapping {
    guard let fallback = options.first else {
        throw ClaudeInstallPanelError.noModels
    }
    let opus = options.first {
        $0.displayName.localizedCaseInsensitiveContains("opus")
    } ?? fallback
    let sonnet = options.first {
        $0.displayName.localizedCaseInsensitiveContains("sonnet")
    } ?? fallback
    let haiku = options.first {
        $0.displayName.localizedCaseInsensitiveContains("haiku")
    } ?? fallback
    return ClaudeModelMapping(opus: opus.id, sonnet: sonnet.id, haiku: haiku.id)
}

private struct InstallClaudeView: View {
    let options: [ClaudeModelOption]
    @State private var opusModel: String
    @State private var sonnetModel: String
    @State private var haikuModel: String
    let cancel: () -> Void
    let install: (ClaudeModelMapping) -> Void

    init(
        options: [ClaudeModelOption],
        initialMapping: ClaudeModelMapping,
        cancel: @escaping () -> Void,
        install: @escaping (ClaudeModelMapping) -> Void
    ) {
        self.options = options
        _opusModel = State(initialValue: initialMapping.opus)
        _sonnetModel = State(initialValue: initialMapping.sonnet)
        _haikuModel = State(initialValue: initialMapping.haiku)
        self.cancel = cancel
        self.install = install
    }

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section {
                    modelPicker(
                        title: "Opus",
                        detail: "Claude Code 选择 Opus 时使用",
                        selection: $opusModel
                    )
                    modelPicker(
                        title: "Sonnet",
                        detail: "默认模型；Claude Code 选择 Sonnet 时使用",
                        selection: $sonnetModel
                    )
                    modelPicker(
                        title: "Haiku",
                        detail: "Claude Code 选择 Haiku 时使用",
                        selection: $haikuModel
                    )
                } header: {
                    Text("选择三个模型映射")
                } footer: {
                    Text("Claude Code 只识别 Opus、Sonnet 和 Haiku 三个模型族；每个模型族会转发到这里选择的实际后端模型。")
                }

                Section {
                    LabeledContent("Claude Code 配置") {
                        Text("~/.claude/settings.json").textSelection(.enabled)
                    }
                    LabeledContent("本地网关") {
                        Text("ANTHROPIC_BASE_URL").textSelection(.enabled)
                    }
                    LabeledContent("自定义模型识别") {
                        Text("modelOverrides").textSelection(.enabled)
                    }
                    LabeledContent("网关认证") {
                        Text("无需登录 Anthropic")
                    }
                    LabeledContent("官方登录入口") {
                        Text("已隐藏")
                    }
                    LabeledContent("非必要流量") {
                        Text("已禁用")
                    }
                } header: {
                    Text("安装配置")
                } footer: {
                    Text("安装时会登记 modelOverrides，并写入 ANTHROPIC_AUTH_TOKEN、DISABLE_LOGIN_COMMAND 和 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC；恢复时还原原值。")
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
                    install(ClaudeModelMapping(
                        opus: opusModel,
                        sonnet: sonnetModel,
                        haiku: haikuModel
                    ))
                }
                .keyboardShortcut(.defaultAction)
                .liquidGlassProminentButton()
            }
            .padding(20)
        }
        .frame(minWidth: 760, minHeight: 480)
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private func modelPicker(
        title: String,
        detail: String,
        selection: Binding<String>
    ) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Picker(title, selection: selection) {
                ForEach(options) { option in
                    Text(option.displayName).tag(option.id)
                }
            }
            .pickerStyle(.menu)
            Text(detail)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding(.vertical, 3)
    }
}

private final class ClaudeModalActionTarget: NSObject {
    let action: () -> Void

    init(_ action: @escaping () -> Void) {
        self.action = action
    }

    @objc func run(_ sender: Any?) {
        action()
    }
}

@MainActor
func runInstallClaudePanel(
    options: [ClaudeModelOption]
) throws -> ClaudeModelMapping? {
    let initialMapping = try suggestedClaudeModelMapping(options: options)
    let panel = NSWindow(
        contentRect: NSRect(x: 0, y: 0, width: 820, height: 540),
        styleMask: [.titled, .closable],
        backing: .buffered,
        defer: false
    )
    panel.title = "安装到 Claude Code"
    panel.level = .normal
    panel.center()
    configurePersistentWindow(panel)

    var confirmedMapping: ClaudeModelMapping?
    let cancel = { NSApp.stopModal(withCode: .cancel) }
    let install = { mapping in
        confirmedMapping = mapping
        NSApp.stopModal(withCode: .OK)
    }
    panel.contentViewController = NSHostingController(
        rootView: InstallClaudeView(
            options: options,
            initialMapping: initialMapping,
            cancel: cancel,
            install: install
        )
    )

    let closeTarget = ClaudeModalActionTarget(cancel)
    panel.standardWindowButton(.closeButton)?.target = closeTarget
    panel.standardWindowButton(.closeButton)?.action = #selector(ClaudeModalActionTarget.run(_:))

    presentPersistentWindow(panel)
    let response = NSApp.runModal(for: panel)
    panel.close()
    _ = closeTarget
    return response == .OK ? confirmedMapping : nil
}
