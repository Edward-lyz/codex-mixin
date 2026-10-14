import Cocoa
import SwiftUI

private struct CodexConnectView: View {
    @State private var selectedMode: AgentConnectMode?
    let cancel: () -> Void
    let connect: (AgentConnectMode) -> Void

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section {
                    Picker(
                        AppLocalization.string("codexConnect.modePickerLabel"),
                        selection: $selectedMode
                    ) {
                        ForEach(AgentConnectMode.allCases, id: \.rawValue) { mode in
                            Text(mode.pickerTitle).tag(Optional(mode))
                        }
                    }
                    .pickerStyle(.radioGroup)
                } header: {
                    Text(AppLocalization.string("codexConnect.modeSectionHeader"))
                } footer: {
                    Text(AppLocalization.string("codexConnect.footer"))
                }

                Section(AppLocalization.string("codexConnect.modesHeader")) {
                    ForEach(AgentConnectMode.allCases, id: \.rawValue) { mode in
                        CodexConnectModeDescription(
                            title: mode.shortLabel,
                            systemImage: mode == .officialAccount
                                ? "person.crop.circle.badge.checkmark"
                                : "network",
                            detail: mode.pickerDetail,
                            isActive: selectedMode == mode
                        )
                    }
                }

                Section(AppLocalization.string("codexConnect.locationHeader")) {
                    LabeledContent(AppLocalization.string("codexConnect.codexConfig")) {
                        Text("~/.codex/config.toml").textSelection(.enabled)
                    }
                    LabeledContent(AppLocalization.string("codexConnect.modelCatalog")) {
                        Text("~/.codex/model-catalogs/mixin-models.json").textSelection(.enabled)
                    }
                    LabeledContent(AppLocalization.string("codexConnect.provider")) {
                        Text(
                            selectedMode?.providerDescription
                                ?? AppLocalization.string("codexConnect.providerPending")
                        )
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
                Button(AppLocalization.string("settings.cancel"), action: cancel)
                    .keyboardShortcut(.cancelAction)
                Button(AppLocalization.string("codexConnect.connectButton")) {
                    if let selectedMode {
                        connect(selectedMode)
                    }
                }
                .keyboardShortcut(.defaultAction)
                .liquidGlassProminentButton()
                .disabled(selectedMode == nil)
            }
            .padding(20)
        }
        .frame(minWidth: 760, minHeight: 510)
        .background(Color(nsColor: .windowBackgroundColor))
    }
}

private struct CodexConnectModeDescription: View {
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

/// The first Codex connection keeps the existing official-account /
/// custom-only choice. The picked mode goes to `agent connect codex --mode`.
@MainActor
func runCodexConnectPanel() -> AgentConnectMode? {
    let panel = NSWindow(
        contentRect: NSRect(x: 0, y: 0, width: 820, height: 560),
        styleMask: [.titled, .closable],
        backing: .buffered,
        defer: false
    )
    panel.title = AppLocalization.string("codexConnect.title")
    panel.level = .normal
    configureOpaqueWindow(panel)
    panel.center()
    configurePersistentWindow(panel)

    var confirmedMode: AgentConnectMode?
    let cancel = { NSApp.stopModal(withCode: .cancel) }
    let connect = { mode in
        confirmedMode = mode
        NSApp.stopModal(withCode: .OK)
    }
    panel.contentViewController = NSHostingController(
        rootView: CodexConnectView(cancel: cancel, connect: connect)
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
