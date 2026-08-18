import Cocoa

final class FusionSettingsWindowController: NSWindowController, NSWindowDelegate, NSTextFieldDelegate {
    typealias LoadHandler = () async throws -> FusionSettingsProfile
    typealias FetchModelsHandler = () async throws -> [FusionModelOption]
    typealias SaveHandler = (FusionSettingsProfile, String, OperationProgress) async throws -> Void
    typealias DeleteHandler = (String, OperationProgress) async throws -> Void

    private let loadHandler: LoadHandler
    private let fetchModelsHandler: FetchModelsHandler
    private let saveHandler: SaveHandler
    private let deleteHandler: DeleteHandler
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
    private let disableButton = NSButton(title: L10n.Fusion.disableButton, target: nil, action: nil)

    init(
        loadHandler: @escaping LoadHandler,
        fetchModelsHandler: @escaping FetchModelsHandler,
        saveHandler: @escaping SaveHandler,
        deleteHandler: @escaping DeleteHandler
    ) {
        self.loadHandler = loadHandler
        self.fetchModelsHandler = fetchModelsHandler
        self.saveHandler = saveHandler
        self.deleteHandler = deleteHandler
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
        disableButton.isEnabled = false
        statusLabel.textColor = .secondaryLabelColor
        statusLabel.stringValue = "正在从本地网关读取模型列表..."
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                loadedProfile = try await loadHandler()
                applyProfileFields()
                let fetched = try await fetchModelsHandler()
                var byId = Dictionary(uniqueKeysWithValues: fetched.map { ($0.id, $0) })
                for id in loadedProfile.panelModels + [loadedProfile.judgeModel, loadedProfile.finalModel] where !id.isEmpty && !id.hasPrefix("mixin/fusion/") {
                    byId[id] = byId[id] ?? FusionModelOption(
                        id: id,
                        displayName: L10n.Fusion.unavailableModel,
                        isAvailable: false
                    )
                }
                options = byId.values.sorted {
                    $0.id.localizedStandardCompare($1.id) == .orderedAscending
                }
                let selection = resolveFusionModelSelection(
                    availableModelIDs: options.map(\.id),
                    storedPanelModels: loadedProfile.panelModels,
                    storedJudgeModel: loadedProfile.judgeModel,
                    storedFinalModel: loadedProfile.finalModel
                )
                loadedProfile.panelModels = selection.panelModels
                loadedProfile.judgeModel = selection.judgeModel
                loadedProfile.finalModel = selection.finalModel
                selectedPanels = Set(selection.panelModels)
                rebuildModelControls()
                statusLabel.stringValue = "已加载 \(options.count) 个跨 Provider 模型。Panel 可选择 1–8 个。"
                statusLabel.textColor = .secondaryLabelColor
                updateValidation()
                updateDisableButton()
            } catch {
                options = []
                rebuildModelControls()
                statusLabel.stringValue = localizedErrorDescription(error)
                statusLabel.textColor = .mixinDegraded
                saveButton.isEnabled = false
                disableButton.isEnabled = false
            }
        }
    }

    private func buildContent(in window: NSWindow) {
        guard let contentView = window.contentView else { return }

        let title = NSTextField(labelWithString: "Fusion 模型编排")
        title.font = .boldSystemFont(ofSize: 20)
        let detail = NSTextField(wrappingLabelWithString: "多个 Panel 模型并行分析，由 Judge 结构化对比，再由 Final 模型流式回答。保存后虚拟模型会出现在 Codex 模型目录中。关闭 Fusion 会删除当前 profile，并从模型选择器中移除 mixin/fusion/<id>。")
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
            planModeLabel(),
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
        disableButton.bezelStyle = .rounded
        disableButton.target = self
        disableButton.action = #selector(disableFusion)
        let cancelButton = NSButton(title: "关闭", target: self, action: #selector(closeWindow))
        cancelButton.bezelStyle = .rounded
        let buttonRow = NSStackView(views: [disableButton, NSView(), cancelButton, saveButton])
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
            let empty = NSTextField(labelWithString: L10n.Fusion.noModels)
            empty.textColor = .secondaryLabelColor
            panelStack.addArrangedSubview(empty)
        } else {
            for option in options {
                let buttonTitle = option.isAvailable
                    ? option.id
                    : "\(option.id)  \(L10n.Fusion.unavailable)"
                let button = NSButton(checkboxWithTitle: buttonTitle, target: self, action: #selector(panelSelectionChanged(_:)))
                button.state = selectedPanels.contains(option.id) ? .on : .off
                button.identifier = NSUserInterfaceItemIdentifier(option.id)
                button.lineBreakMode = .byTruncatingMiddle
                button.toolTip = "\(option.id)\n\(option.displayName)"
                panelButtons[option.id] = button

                let detail = NSTextField(labelWithString: option.displayName)
                detail.textColor = option.isAvailable ? .secondaryLabelColor : .mixinDegraded
                detail.font = .systemFont(ofSize: 11)
                detail.lineBreakMode = .byTruncatingTail
                detail.toolTip = option.displayName

                let row = NSStackView(views: [button, detail])
                row.orientation = .vertical
                row.alignment = .leading
                row.spacing = 1
                row.translatesAutoresizingMaskIntoConstraints = false
                row.widthAnchor.constraint(equalToConstant: 690).isActive = true
                panelStack.addArrangedSubview(row)
            }
        }
        configurePopup(judgePopup, selected: loadedProfile.judgeModel)
        configurePopup(finalPopup, selected: loadedProfile.finalModel.isEmpty ? loadedProfile.judgeModel : loadedProfile.finalModel)
    }

    private func configurePopup(_ popup: NSPopUpButton, selected: String) {
        popup.removeAllItems()
        for option in options {
            let title = option.isAvailable
                ? option.id
                : "\(option.id) \(L10n.Fusion.unavailable)"
            popup.addItem(withTitle: title)
            popup.lastItem?.representedObject = option.id
            popup.lastItem?.toolTip = "\(option.id)\n\(option.displayName)"
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
                statusLabel.textColor = .mixinDegraded
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
            statusLabel.textColor = .mixinDegraded
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
        disableButton.isEnabled = false
        statusLabel.stringValue = "正在保存并重启本地网关..."
        statusLabel.textColor = .controlAccentColor
        let replacedProfileID = loadedProfile.id
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await runOperationProgress(
                    title: "正在保存 Fusion 配置",
                    phases: [
                        "写入 Fusion 配置",
                        "重启本地网关",
                        "刷新 Codex 模型目录",
                        "完成",
                    ],
                    successTitle: "✓ Fusion 配置已保存",
                    failureTitle: "✗ 保存失败",
                    showFailureAlert: false
                ) { progress in
                    try await self.saveHandler(profile, replacedProfileID, progress)
                }
                loadedProfile = profile
                statusLabel.stringValue = L10n.Fusion.saveSuccess(profile.id)
                statusLabel.textColor = .mixinHealthy
                saveButton.isEnabled = true
                updateDisableButton()
            } catch {
                statusLabel.stringValue = L10n.Fusion.saveFailed(localizedErrorDescription(error))
                statusLabel.textColor = .mixinError
                saveButton.isEnabled = true
                updateDisableButton()
                presentFusionAlert(
                    title: L10n.Fusion.saveAlertTitle,
                    message: localizedErrorDescription(error)
                )
            }
        }
    }

    @objc private func disableFusion() {
        let profileID = loadedProfile.id.trimmingCharacters(in: .whitespacesAndNewlines)
        guard hasConfiguredProfile() else { return }
        guard confirm(
            title: L10n.Fusion.disableConfirmTitle,
            message: L10n.Fusion.disableConfirmMessage(profileID)
        ) else { return }
        saveButton.isEnabled = false
        disableButton.isEnabled = false
        statusLabel.stringValue = "正在关闭 Fusion 并从模型目录移除..."
        statusLabel.textColor = .controlAccentColor
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await runOperationProgress(
                    title: "正在关闭 Fusion",
                    phases: [
                        "删除 Fusion 配置",
                        "重启本地网关",
                        "刷新 Codex 模型目录",
                        "完成",
                    ],
                    successTitle: "✓ Fusion 已关闭",
                    failureTitle: "✗ 关闭失败",
                    showFailureAlert: false
                ) { progress in
                    try await self.deleteHandler(profileID, progress)
                }
                loadedProfile = FusionSettingsProfile()
                applyProfileFields()
                selectedPanels.removeAll()
                rebuildModelControls()
                statusLabel.stringValue = L10n.Fusion.disableSuccess(profileID)
                statusLabel.textColor = .mixinHealthy
                updateValidation()
                updateDisableButton()
            } catch {
                statusLabel.stringValue = L10n.Fusion.disableFailed(localizedErrorDescription(error))
                statusLabel.textColor = .mixinError
                updateValidation()
                updateDisableButton()
                presentFusionAlert(
                    title: L10n.Fusion.disableAlertTitle,
                    message: localizedErrorDescription(error)
                )
            }
        }
    }

    private func hasConfiguredProfile() -> Bool {
        let profileID = loadedProfile.id.trimmingCharacters(in: .whitespacesAndNewlines)
        return !profileID.isEmpty
            && !(loadedProfile.panelModels.isEmpty
                && loadedProfile.judgeModel.isEmpty
                && loadedProfile.finalModel.isEmpty)
    }

    private func updateDisableButton() {
        disableButton.isEnabled = hasConfiguredProfile()
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

private func planModeLabel() -> NSTextField {
    let label = NSTextField(wrappingLabelWithString: "Fusion 仅在 Plan 模式的新用户轮次运行；进入 Default 执行模式后，后续请求与工具结果续跑均直接交给 Final 模型。")
    label.textColor = .secondaryLabelColor
    label.translatesAutoresizingMaskIntoConstraints = false
    label.widthAnchor.constraint(equalToConstant: 680).isActive = true
    return label
}

private func presentFusionAlert(title: String, message: String) {
    let alert = NSAlert()
    alert.messageText = localizedPrompt(title)
    alert.informativeText = localizedGatewayMessage(message)
    alert.alertStyle = .warning
    alert.addButton(withTitle: L10n.Common.ok)
    NSApp.activate(ignoringOtherApps: true)
    alert.runModal()
}
