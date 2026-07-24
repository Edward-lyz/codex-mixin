import Cocoa

enum FusionSettingsError: Error, CustomStringConvertible {
    case message(String)

    var description: String {
        switch self {
        case .message(let message): return message
        }
    }
}

struct FusionModelOption: Hashable {
    let id: String
    let displayName: String
}

struct FusionSettingsProfile {
    var id = "default"
    var panelModels: [String] = []
    var judgeModel = ""
    var finalModel = ""
    var minSuccessful = 1
    var maxCompletionTokens = 2048
    var timeoutMs = 300_000
    var showIntermediateResults = true
    var panelToolsEnabled = true
    var panelMaxRounds = 16
    var panelMaxCallsPerModel = 64

    static func fromCLIJSON(_ rawJSON: String) throws -> FusionSettingsProfile {
        let data = Data(rawJSON.utf8)
        guard
            let envelope = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            throw FusionSettingsError.message("Fusion CLI 返回了无效 JSON")
        }
        guard let profile = envelope["profile"] as? [String: Any] else {
            return FusionSettingsProfile()
        }
        var value = FusionSettingsProfile()
        value.id = profile["id"] as? String ?? value.id
        value.panelModels = profile["panel_models"] as? [String] ?? value.panelModels
        value.judgeModel = profile["judge_model"] as? String ?? value.judgeModel
        value.finalModel = profile["final_model"] as? String ?? value.finalModel
        value.minSuccessful = (profile["min_successful"] as? NSNumber)?.intValue ?? value.minSuccessful
        value.maxCompletionTokens = (profile["max_completion_tokens"] as? NSNumber)?.intValue ?? value.maxCompletionTokens
        value.timeoutMs = (profile["timeout_ms"] as? NSNumber)?.intValue ?? value.timeoutMs
        value.showIntermediateResults = (profile["show_intermediate_results"] as? NSNumber)?.boolValue ?? value.showIntermediateResults
        if let tools = profile["panel_tools"] as? [String: Any] {
            value.panelToolsEnabled = (tools["enabled"] as? NSNumber)?.boolValue ?? value.panelToolsEnabled
            let storedRounds = (tools["max_rounds"] as? NSNumber)?.intValue
            let storedCalls = (tools["max_calls_per_model"] as? NSNumber)?.intValue
            // Automatically migrate the original, overly restrictive defaults.
            value.panelMaxRounds = storedRounds == 4 ? 16 : (storedRounds ?? value.panelMaxRounds)
            value.panelMaxCallsPerModel = storedCalls == 8 ? 64 : (storedCalls ?? value.panelMaxCallsPerModel)
        }
        return value
    }

    var dictionary: [String: Any] {
        [
            "id": id,
            "panel_models": panelModels,
            "judge_model": judgeModel,
            "final_model": finalModel,
            "min_successful": minSuccessful,
            "max_completion_tokens": maxCompletionTokens,
            "timeout_ms": timeoutMs,
            "fuse_every_user_turn": true,
            "show_intermediate_results": showIntermediateResults,
            "panel_tools": [
                "enabled": panelToolsEnabled,
                "max_rounds": panelMaxRounds,
                "max_calls_per_model": panelMaxCallsPerModel,
            ],
        ]
    }

    func jsonString() throws -> String {
        let data = try JSONSerialization.data(
            withJSONObject: dictionary,
            options: [.sortedKeys]
        )
        guard let value = String(data: data, encoding: .utf8) else {
            throw FusionSettingsError.message("Fusion 配置无法编码为 UTF-8 JSON")
        }
        return value
    }
}

final class FusionSettingsWindowController: NSWindowController, NSWindowDelegate, NSTextFieldDelegate {
    typealias LoadHandler = () async throws -> FusionSettingsProfile
    typealias FetchModelsHandler = () async throws -> [FusionModelOption]
    typealias SaveHandler = (FusionSettingsProfile, String) async throws -> Void

    private let loadHandler: LoadHandler
    private let fetchModelsHandler: FetchModelsHandler
    private let saveHandler: SaveHandler
    private var loadedProfile = FusionSettingsProfile()
    private var options: [FusionModelOption] = []
    private var panelButtons: [String: NSButton] = [:]
    private var selectedPanels: Set<String> = []

    private let profileIdField = NSTextField()
    private let panelStack = NSStackView()
    private let judgePopup = NSPopUpButton()
    private let finalPopup = NSPopUpButton()
    private let minSuccessfulField = NSTextField()
    private let timeoutField = NSTextField()
    private let resultsCheckbox = NSButton(checkboxWithTitle: "在回答中显示 Panel / Judge 中间结果", target: nil, action: nil)
    private let toolsCheckbox = NSButton(checkboxWithTitle: "允许 Panel 使用进程内只读工具", target: nil, action: nil)
    private let statusLabel = NSTextField(wrappingLabelWithString: "正在读取配置...")
    private let saveButton = NSButton(title: "保存并重启网关", target: nil, action: nil)

    init(
        loadHandler: @escaping LoadHandler,
        fetchModelsHandler: @escaping FetchModelsHandler,
        saveHandler: @escaping SaveHandler
    ) {
        self.loadHandler = loadHandler
        self.fetchModelsHandler = fetchModelsHandler
        self.saveHandler = saveHandler
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 820, height: 700),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Fusion 设置"
        window.minSize = NSSize(width: 700, height: 580)
        window.isReleasedWhenClosed = false
        window.center()
        super.init(window: window)
        window.delegate = self
        buildContent(in: window)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present() {
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        reload()
    }

    private func reload() {
        saveButton.isEnabled = false
        statusLabel.textColor = .secondaryLabelColor
        statusLabel.stringValue = "正在从本地网关读取模型列表..."
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                loadedProfile = try await loadHandler()
                applyProfileFields()
                let fetched = try await fetchModelsHandler()
                selectedPanels = Set(loadedProfile.panelModels)
                var byId = Dictionary(uniqueKeysWithValues: fetched.map { ($0.id, $0) })
                for id in loadedProfile.panelModels + [loadedProfile.judgeModel, loadedProfile.finalModel] where !id.isEmpty && !id.hasPrefix("mixin/fusion/") {
                    byId[id] = byId[id] ?? FusionModelOption(id: id, displayName: id)
                }
                options = byId.values.sorted {
                    $0.displayName.localizedStandardCompare($1.displayName) == .orderedAscending
                }
                rebuildModelControls()
                statusLabel.stringValue = "已加载 \(options.count) 个跨 Provider 模型。Panel 可选择 1–8 个。"
                statusLabel.textColor = .secondaryLabelColor
                updateValidation()
            } catch {
                options = []
                rebuildModelControls()
                statusLabel.stringValue = String(describing: error)
                statusLabel.textColor = .systemOrange
                saveButton.isEnabled = false
            }
        }
    }

    private func buildContent(in window: NSWindow) {
        guard let contentView = window.contentView else { return }

        let title = NSTextField(labelWithString: "Fusion 模型编排")
        title.font = .boldSystemFont(ofSize: 20)
        let detail = NSTextField(wrappingLabelWithString: "多个 Panel 模型并行分析，由 Judge 结构化对比，再由 Final 模型流式回答。保存后虚拟模型会出现在 Codex 模型目录中。")
        detail.textColor = .secondaryLabelColor

        configureTextField(profileIdField, value: "default")
        configureTextField(minSuccessfulField, value: "1")
        configureTextField(timeoutField, value: "300000")
        profileIdField.delegate = self
        minSuccessfulField.delegate = self
        timeoutField.delegate = self

        panelStack.orientation = .vertical
        panelStack.alignment = .leading
        panelStack.spacing = 6
        panelStack.edgeInsets = NSEdgeInsets(top: 8, left: 10, bottom: 8, right: 10)
        let panelDocument = NSView()
        panelDocument.translatesAutoresizingMaskIntoConstraints = false
        panelStack.translatesAutoresizingMaskIntoConstraints = false
        panelDocument.addSubview(panelStack)
        NSLayoutConstraint.activate([
            panelStack.leadingAnchor.constraint(equalTo: panelDocument.leadingAnchor),
            panelStack.trailingAnchor.constraint(equalTo: panelDocument.trailingAnchor),
            panelStack.topAnchor.constraint(equalTo: panelDocument.topAnchor),
            panelStack.bottomAnchor.constraint(equalTo: panelDocument.bottomAnchor),
        ])
        let panelScroll = NSScrollView()
        panelScroll.documentView = panelDocument
        panelScroll.hasVerticalScroller = true
        panelScroll.autohidesScrollers = true
        panelScroll.borderType = .bezelBorder
        panelScroll.translatesAutoresizingMaskIntoConstraints = false
        panelScroll.heightAnchor.constraint(equalToConstant: 230).isActive = true
        panelScroll.widthAnchor.constraint(equalToConstant: 730).isActive = true
        panelDocument.widthAnchor.constraint(equalTo: panelScroll.contentView.widthAnchor).isActive = true

        judgePopup.target = self
        judgePopup.action = #selector(controlChanged)
        finalPopup.target = self
        finalPopup.action = #selector(controlChanged)
        resultsCheckbox.target = self
        resultsCheckbox.action = #selector(controlChanged)
        toolsCheckbox.target = self
        toolsCheckbox.action = #selector(controlChanged)

        let advanced = NSBox()
        advanced.title = "高级选项"
        let advancedStack = NSStackView(views: [
            settingsRow("最少成功 Panel", minSuccessfulField),
            settingsRow("单模型超时 (ms)", timeoutField),
            everyUserTurnLabel(),
            resultsCheckbox,
            toolsCheckbox,
        ])
        advancedStack.orientation = .vertical
        advancedStack.alignment = .leading
        advancedStack.spacing = 10
        advancedStack.translatesAutoresizingMaskIntoConstraints = false
        advanced.contentView?.addSubview(advancedStack)
        if let advancedContent = advanced.contentView {
            NSLayoutConstraint.activate([
                advancedStack.leadingAnchor.constraint(equalTo: advancedContent.leadingAnchor, constant: 12),
                advancedStack.trailingAnchor.constraint(equalTo: advancedContent.trailingAnchor, constant: -12),
                advancedStack.topAnchor.constraint(equalTo: advancedContent.topAnchor, constant: 10),
                advancedStack.bottomAnchor.constraint(equalTo: advancedContent.bottomAnchor, constant: -12),
            ])
        }
        advanced.translatesAutoresizingMaskIntoConstraints = false
        advanced.widthAnchor.constraint(equalToConstant: 730).isActive = true

        saveButton.bezelStyle = .rounded
        saveButton.keyEquivalent = "\r"
        saveButton.target = self
        saveButton.action = #selector(save)
        let cancelButton = NSButton(title: "关闭", target: self, action: #selector(closeWindow))
        cancelButton.bezelStyle = .rounded
        let buttonRow = NSStackView(views: [NSView(), cancelButton, saveButton])
        buttonRow.orientation = .horizontal
        buttonRow.spacing = 10

        let stack = NSStackView(views: [
            title,
            detail,
            settingsRow("Profile ID", profileIdField),
            sectionLabel("Panel 模型（多选）"),
            panelScroll,
            settingsRow("Judge 模型", judgePopup),
            settingsRow("Final 模型", finalPopup),
            advanced,
            statusLabel,
            buttonRow,
        ])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false
        contentView.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 28),
            stack.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -28),
            stack.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 24),
            stack.bottomAnchor.constraint(lessThanOrEqualTo: contentView.bottomAnchor, constant: -22),
            detail.widthAnchor.constraint(equalTo: stack.widthAnchor),
            statusLabel.widthAnchor.constraint(equalTo: stack.widthAnchor),
            buttonRow.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])
    }

    private func applyProfileFields() {
        profileIdField.stringValue = loadedProfile.id
        minSuccessfulField.stringValue = String(loadedProfile.minSuccessful)
        timeoutField.stringValue = String(loadedProfile.timeoutMs)
        resultsCheckbox.state = loadedProfile.showIntermediateResults ? .on : .off
        toolsCheckbox.state = loadedProfile.panelToolsEnabled ? .on : .off
        selectedPanels = Set(loadedProfile.panelModels)
    }

    private func rebuildModelControls() {
        panelStack.arrangedSubviews.forEach {
            panelStack.removeArrangedSubview($0)
            $0.removeFromSuperview()
        }
        panelButtons.removeAll()
        if options.isEmpty {
            let empty = NSTextField(labelWithString: "没有可用模型")
            empty.textColor = .secondaryLabelColor
            panelStack.addArrangedSubview(empty)
        } else {
            for option in options {
                let title = option.displayName == option.id
                    ? option.id
                    : "\(option.displayName)  ·  \(option.id)"
                let button = NSButton(checkboxWithTitle: title, target: self, action: #selector(panelSelectionChanged(_:)))
                button.state = selectedPanels.contains(option.id) ? .on : .off
                button.identifier = NSUserInterfaceItemIdentifier(option.id)
                button.lineBreakMode = .byTruncatingMiddle
                button.toolTip = option.id
                panelButtons[option.id] = button
                panelStack.addArrangedSubview(button)
            }
        }
        configurePopup(judgePopup, selected: loadedProfile.judgeModel)
        configurePopup(finalPopup, selected: loadedProfile.finalModel.isEmpty ? loadedProfile.judgeModel : loadedProfile.finalModel)
    }

    private func configurePopup(_ popup: NSPopUpButton, selected: String) {
        popup.removeAllItems()
        for option in options {
            popup.addItem(withTitle: option.displayName)
            popup.lastItem?.representedObject = option.id
            popup.lastItem?.toolTip = option.id
        }
        if let index = popup.itemArray.firstIndex(where: { ($0.representedObject as? String) == selected }) {
            popup.selectItem(at: index)
        } else if !options.isEmpty {
            popup.selectItem(at: 0)
        }
    }

    @objc private func panelSelectionChanged(_ sender: NSButton) {
        guard let id = sender.identifier?.rawValue else { return }
        if sender.state == .on {
            if selectedPanels.count >= 8 {
                sender.state = .off
                statusLabel.stringValue = "Panel 最多选择 8 个模型。"
                statusLabel.textColor = .systemOrange
            } else {
                selectedPanels.insert(id)
            }
        } else {
            selectedPanels.remove(id)
        }
        updateValidation()
    }

    @objc private func controlChanged() {
        updateValidation()
    }

    func controlTextDidChange(_ obj: Notification) {
        updateValidation()
    }

    private func validationError() -> String? {
        let id = profileIdField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        if id.isEmpty || id.contains("/") {
            return "Profile ID 不能为空且不能包含 /。"
        }
        if !(1...8).contains(selectedPanels.count) {
            return "请选择 1–8 个 Panel 模型。"
        }
        guard
            let judge = selectedModel(judgePopup),
            let final = selectedModel(finalPopup)
        else { return "Judge 和 Final 模型不能为空。" }
        if judge.hasPrefix("mixin/fusion/") || final.hasPrefix("mixin/fusion/") || selectedPanels.contains(where: { $0.hasPrefix("mixin/fusion/") }) {
            return "Fusion profile 不能递归引用 mixin/fusion/ 模型。"
        }
        guard let minimum = Int(minSuccessfulField.stringValue), (1...selectedPanels.count).contains(minimum) else {
            return "min_successful 必须在 1 和 Panel 数量之间。"
        }
        guard let timeout = Int(timeoutField.stringValue), timeout > 0 else {
            return "timeout_ms 必须大于 0。"
        }
        return nil
    }

    private func updateValidation() {
        if let error = validationError() {
            saveButton.isEnabled = false
            statusLabel.stringValue = error
            statusLabel.textColor = .systemOrange
        } else {
            saveButton.isEnabled = !options.isEmpty
            statusLabel.stringValue = "配置有效：已选择 \(selectedPanels.count) 个 Panel 模型。"
            statusLabel.textColor = .secondaryLabelColor
        }
    }

    @objc private func save() {
        guard validationError() == nil else {
            updateValidation()
            return
        }
        var profile = loadedProfile
        profile.id = profileIdField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        profile.panelModels = options.map(\.id).filter(selectedPanels.contains)
        profile.judgeModel = selectedModel(judgePopup) ?? ""
        profile.finalModel = selectedModel(finalPopup) ?? ""
        profile.minSuccessful = Int(minSuccessfulField.stringValue) ?? 1
        profile.timeoutMs = Int(timeoutField.stringValue) ?? 300_000
        profile.showIntermediateResults = resultsCheckbox.state == .on
        profile.panelToolsEnabled = toolsCheckbox.state == .on
        saveButton.isEnabled = false
        statusLabel.stringValue = "正在保存并重启本地网关..."
        statusLabel.textColor = .controlAccentColor
        let replacedProfileID = loadedProfile.id
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await saveHandler(profile, replacedProfileID)
                loadedProfile = profile
                statusLabel.stringValue = "保存成功。虚拟模型 mixin/fusion/\(profile.id) 已写入 catalog。"
                statusLabel.textColor = .systemGreen
                saveButton.isEnabled = true
            } catch {
                statusLabel.stringValue = "保存失败：\(error)"
                statusLabel.textColor = .systemRed
                saveButton.isEnabled = true
                presentFusionAlert(title: "保存 Fusion 设置失败", message: String(describing: error))
            }
        }
    }

    private func selectedModel(_ popup: NSPopUpButton) -> String? {
        popup.selectedItem?.representedObject as? String
    }

    @objc private func closeWindow() {
        window?.close()
    }
}

private func configureTextField(_ field: NSTextField, value: String) {
    field.stringValue = value
    field.controlSize = .regular
    field.translatesAutoresizingMaskIntoConstraints = false
    field.widthAnchor.constraint(equalToConstant: 430).isActive = true
}

private func settingsRow(_ title: String, _ control: NSView) -> NSView {
    let label = NSTextField(labelWithString: title)
    label.textColor = .secondaryLabelColor
    label.alignment = .right
    label.translatesAutoresizingMaskIntoConstraints = false
    label.widthAnchor.constraint(equalToConstant: 145).isActive = true
    control.translatesAutoresizingMaskIntoConstraints = false
    if control is NSPopUpButton {
        control.widthAnchor.constraint(equalToConstant: 520).isActive = true
    }
    let row = NSStackView(views: [label, control])
    row.orientation = .horizontal
    row.alignment = .centerY
    row.spacing = 10
    return row
}

private func sectionLabel(_ title: String) -> NSTextField {
    let label = NSTextField(labelWithString: title)
    label.font = .systemFont(ofSize: 13, weight: .semibold)
    return label
}

private func everyUserTurnLabel() -> NSTextField {
    let label = NSTextField(wrappingLabelWithString: "Fusion 会在每个新用户轮次运行，包括 Plan 与后续写代码阶段；同一轮的工具结果续跑直接交给 Final 模型。")
    label.textColor = .secondaryLabelColor
    label.translatesAutoresizingMaskIntoConstraints = false
    label.widthAnchor.constraint(equalToConstant: 680).isActive = true
    return label
}

private func presentFusionAlert(title: String, message: String) {
    let alert = NSAlert()
    alert.messageText = title
    alert.informativeText = message
    alert.alertStyle = .warning
    alert.addButton(withTitle: "确定")
    NSApp.activate(ignoringOtherApps: true)
    alert.runModal()
}
