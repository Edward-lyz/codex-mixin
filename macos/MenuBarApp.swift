import Cocoa

final class AppDelegate: NSObject, NSApplicationDelegate {
    private let serviceLabel = "local.codex-mixin.service"
    private let autoUpdateCheckKey = "autoCheckUpdates"
    private let lastUpdateCheckKey = "lastUpdateCheckAt"
    private var statusItem: NSStatusItem?
    private var serviceStatusItem: NSMenuItem?
    private var quotaStatusItem: NSMenuItem?
    private var startMenuItem: NSMenuItem?
    private var stopMenuItem: NSMenuItem?
    private var restartMenuItem: NSMenuItem?
    private var launchAtLoginMenuItem: NSMenuItem?
    private var autoUpdateMenuItem: NSMenuItem?
    private var timer: Timer?
    private var isRunning = false
    private var serviceBusy = false {
        didSet { updateActionStates() }
    }
    private var serviceStatus = "服务：检查中..." {
        didSet { serviceStatusItem?.title = serviceStatus }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        installApplicationMenu()
        installStatusItem()
        refreshStatus()
        timer = Timer.scheduledTimer(withTimeInterval: 60, repeats: true) { [weak self] _ in
            Task { @MainActor in
                self?.refreshStatus()
            }
        }
        if autoUpdateChecksEnabled() {
            DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak self] in
                self?.checkForUpdatesFromAutoCheck()
            }
        }
        if CommandLine.arguments.contains("--show-settings") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
                self?.configureLogin()
            }
        }
        if CommandLine.arguments.contains("--check-updates") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
                self?.checkForUpdatesFromMenu()
            }
        }
    }

    private func installApplicationMenu() {
        let mainMenu = NSMenu()
        let appMenuItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "退出", action: #selector(quit), keyEquivalent: "q").target = self
        appMenuItem.submenu = appMenu
        mainMenu.addItem(appMenuItem)

        let editMenuItem = NSMenuItem()
        let editMenu = NSMenu(title: "编辑")
        editMenu.addItem(withTitle: "剪切", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "复制", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "粘贴", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: "全选", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editMenuItem.submenu = editMenu
        mainMenu.addItem(editMenuItem)
        NSApp.mainMenu = mainMenu
    }

    private func installStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.title = ""
        item.button?.toolTip = "Codex Mixin"
        item.button?.image = codexStatusImage(isRunning: false)
        item.button?.imagePosition = .imageOnly
        item.menu = buildMenu()
        statusItem = item
    }

    private func buildMenu() -> NSMenu {
        let menu = NSMenu()
        let serviceItem = NSMenuItem(title: serviceStatus, action: nil, keyEquivalent: "")
        serviceItem.isEnabled = false
        serviceItem.image = menuItemImage("bolt.horizontal.circle")
        let quotaItem = NSMenuItem(title: "", action: nil, keyEquivalent: "")
        quotaItem.isEnabled = false
        quotaItem.image = menuItemImage("chart.bar")
        serviceStatusItem = serviceItem
        quotaStatusItem = quotaItem
        menu.addItem(serviceItem)
        menu.addItem(quotaItem)
        updateQuotaStatus(title: "额度：检查中...", detail: nil, progress: nil)
        menu.addItem(.separator())
        startMenuItem = actionItem("启动本地网关", #selector(startService), "play.circle")
        stopMenuItem = actionItem("暂停本地网关", #selector(stopService), "pause.circle")
        restartMenuItem = actionItem("重启本地网关", #selector(restartService), "arrow.clockwise.circle")
        menu.addItem(startMenuItem!)
        menu.addItem(stopMenuItem!)
        menu.addItem(restartMenuItem!)
        launchAtLoginMenuItem = actionItem("登录时启动并开启服务", #selector(toggleLaunchAtLogin), "poweron")
        menu.addItem(launchAtLoginMenuItem!)
        menu.addItem(actionItem("刷新状态与额度", #selector(refreshStatus), "arrow.clockwise"))
        menu.addItem(.separator())
        menu.addItem(actionItem("设置供应商与密钥...", #selector(configureLogin), "gearshape"))
        menu.addItem(actionItem("安装到 Codex...", #selector(installCodexConfig), "square.and.arrow.down"))
        menu.addItem(actionItem("从 Codex 恢复...", #selector(uninstallCodexConfig), "arrow.uturn.backward.circle"))
        menu.addItem(.separator())
        menu.addItem(actionItem("检查更新...", #selector(checkForUpdatesFromMenu), "arrow.down.circle"))
        autoUpdateMenuItem = actionItem("自动检查更新", #selector(toggleAutoUpdateChecks), "clock.arrow.circlepath")
        menu.addItem(autoUpdateMenuItem!)
        menu.addItem(.separator())
        menu.addItem(actionItem("复制本地接口地址", #selector(copyLocalEndpoint), "link"))
        menu.addItem(actionItem("打开运行日志", #selector(openLogs), "doc.text"))
        menu.addItem(actionItem("打开配置目录", #selector(openConfigFolder), "folder"))
        menu.addItem(.separator())
        menu.addItem(actionItem("退出 Codex Mixin", #selector(quit), "power"))
        updateActionStates()
        return menu
    }

    private func actionItem(_ title: String, _ action: Selector, _ symbolName: String) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
        item.target = self
        item.image = menuItemImage(symbolName)
        return item
    }

    private func updateStatusTitle() {
        statusItem?.button?.image = codexStatusImage(isRunning: isRunning)
        statusItem?.button?.toolTip = isRunning ? "Codex Mixin：运行中" : "Codex Mixin：已停止"
    }

    private func updateActionStates() {
        startMenuItem?.isEnabled = !serviceBusy && !isRunning
        stopMenuItem?.isEnabled = !serviceBusy && isRunning
        restartMenuItem?.isEnabled = !serviceBusy
        launchAtLoginMenuItem?.state = FileManager.default.fileExists(atPath: launchAgentPath().path) ? .on : .off
        autoUpdateMenuItem?.state = autoUpdateChecksEnabled() ? .on : .off
    }

    @objc private func startService() {
        serviceStatus = "服务：启动中..."
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                try installLaunchAgent()
                try await bootoutIfLoaded()
                _ = try await runProcess("/bin/launchctl", ["bootstrap", launchDomain(), launchAgentPath().path])
                refreshStatus()
            } catch {
                serviceStatus = "服务：启动失败"
                showAlert(title: "启动服务失败", message: String(describing: error))
            }
        }
    }

    @objc private func stopService() {
        serviceStatus = "服务：停止中..."
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                try await bootoutIfLoaded()
                _ = try? await runGateway(["stop"])
                refreshStatus()
            } catch {
                serviceStatus = "服务：停止失败"
                showAlert(title: "暂停服务失败", message: String(describing: error))
            }
        }
    }

    @objc private func restartService() {
        serviceStatus = "服务：重启中..."
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                try await bootoutIfLoaded()
                try installLaunchAgent()
                _ = try await runProcess("/bin/launchctl", ["bootstrap", launchDomain(), launchAgentPath().path])
                refreshStatus()
            } catch {
                serviceStatus = "服务：重启失败"
                showAlert(title: "重启服务失败", message: String(describing: error))
            }
        }
    }

    @objc private func toggleLaunchAtLogin() {
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                if FileManager.default.fileExists(atPath: launchAgentPath().path) {
                    try await bootoutIfLoaded()
                    _ = try? await runGateway(["stop"])
                    try FileManager.default.removeItem(at: launchAgentPath())
                } else {
                    try installLaunchAgent()
                    try await bootoutIfLoaded()
                    _ = try await runProcess("/bin/launchctl", ["bootstrap", launchDomain(), launchAgentPath().path])
                }
                refreshStatus()
            } catch {
                showAlert(title: "更新登录自启失败", message: String(describing: error))
            }
        }
    }

    @objc private func refreshStatus() {
        Task { @MainActor in
            let launchdLoaded = (try? await runProcess("/bin/launchctl", ["print", launchDomainAndLabel()])) != nil
            let cliStatus = try? await runGateway(["status"])
            let cliRunning = cliStatus?.contains("gateway: running") == true
            isRunning = launchdLoaded || cliRunning
            updateStatusTitle()
            updateActionStates()
            serviceStatus = isRunning ? "服务：运行中" : "服务：已停止"

            do {
                let quota = try await runGateway(["quota", "--json"])
                let usage = try parseQuotaUsage(quota)
                let title: String
                let detail: String?
                let progress: Double?
                if let limit = usage.limit {
                    title = "额度：已用 \(formatQuotaAmount(usage.used)) / \(formatQuotaAmount(limit))"
                    detail = usage.remaining.map { "剩余 \(formatQuotaAmount($0))" }
                    progress = limit > 0 ? usage.used / limit : nil
                } else {
                    title = "额度：已用 \(formatQuotaAmount(usage.used))"
                    detail = nil
                    progress = nil
                }
                updateQuotaStatus(
                    title: title,
                    detail: detail,
                    progress: progress
                )
            } catch {
                let message = String(describing: error)
                if message.contains("quota URL") {
                    updateQuotaStatus(title: "额度：未配置接口", detail: nil, progress: nil)
                } else if message.contains("auth key") {
                    updateQuotaStatus(title: "额度：未配置密钥", detail: nil, progress: nil)
                } else {
                    updateQuotaStatus(title: "额度：不可用", detail: nil, progress: nil)
                }
            }
        }
    }

    private func updateQuotaStatus(title: String, detail: String?, progress: Double?) {
        quotaStatusItem?.view = quotaMenuView(title: title, detail: detail, progress: progress)
    }

    @objc private func configureLogin() {
        let stored: [String: Any]
        do {
            stored = try loadStoredConfig()
        } catch {
            showAlert(title: "读取配置失败", message: String(describing: error))
            return
        }
        guard let values = runLoginSettingsPanel(stored: stored) else { return }
        let apiKey = values.apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        if apiKey.isEmpty && stored["upstream_api_key"] == nil {
            showAlert(title: "缺少 API 密钥", message: "首次配置必须填写上游 API 密钥。")
            return
        }
        if values.provider == "custom" && values.baseUrl.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && stored["upstream_base_url"] == nil {
            showAlert(title: "缺少上游地址", message: "Custom 模式必须填写上游服务根地址。")
            return
        }
        var args = ["login"]
        appendOptionalArg(&args, "--provider", values.provider)
        appendOptionalArg(&args, "--key", apiKey)
        appendOptionalArg(&args, "--base-url", values.baseUrl)
        appendOptionalArg(&args, "--gateway-key", values.gatewayKey)
        appendOptionalArg(&args, "--quota-url", values.quotaUrl)
        appendOptionalArg(&args, "--quota-username", values.quotaUsername)
        Task { @MainActor in
            do {
                let output = try await runGateway(args)
                showAlert(title: "设置已保存", message: output.isEmpty ? "完成" : output)
                refreshStatus()
            } catch {
                showAlert(title: "保存设置失败", message: String(describing: error))
            }
        }
    }

    @objc private func installCodexConfig() {
        guard runInstallCodexPanel() else { return }
        let args = ["install-codex", "--codex-oauth-proxy"]
        Task { @MainActor in
            do {
                let output = try await runGateway(args)
                let message = output.isEmpty ? "已写入托管模型配置。请重启 Codex App；CLI 需要开新会话。" : "\(output)\n\n请重启 Codex App；CLI 需要开新会话。"
                showAlert(title: "Codex 配置已更新", message: message)
                refreshStatus()
            } catch {
                showAlert(title: "安装到 Codex 失败", message: String(describing: error))
            }
        }
    }

    @objc private func uninstallCodexConfig() {
        guard confirm(title: "从 Codex 恢复官方配置", message: "会恢复安装前备份的 ~/.codex/config.toml，并删除 Codex Mixin 托管的模型目录。完成后需要重启 Codex App；CLI 需要开新会话。") else { return }
        Task { @MainActor in
            do {
                let output = try await runGateway(["uninstall-codex"])
                let message = output.isEmpty ? "已恢复安装前配置。请重启 Codex App；CLI 需要开新会话。" : "\(output)\n\n请重启 Codex App；CLI 需要开新会话。"
                showAlert(title: "Codex 配置已恢复", message: message)
                refreshStatus()
            } catch {
                showAlert(title: "从 Codex 恢复失败", message: String(describing: error))
            }
        }
    }

    @objc private func copyLocalEndpoint() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString("http://127.0.0.1:8787/v1", forType: .string)
    }

    @objc private func checkForUpdatesFromMenu() {
        Task { @MainActor in
            await checkForUpdates(interactive: true)
        }
    }

    private func checkForUpdatesFromAutoCheck() {
        let now = Date()
        let lastCheck = UserDefaults.standard.object(forKey: lastUpdateCheckKey) as? Date
        if let lastCheck, now.timeIntervalSince(lastCheck) < 24 * 60 * 60 {
            return
        }
        UserDefaults.standard.set(now, forKey: lastUpdateCheckKey)
        Task { @MainActor in
            await checkForUpdates(interactive: false)
        }
    }

    @objc private func toggleAutoUpdateChecks() {
        let next = !autoUpdateChecksEnabled()
        UserDefaults.standard.set(next, forKey: autoUpdateCheckKey)
        updateActionStates()
        if next {
            checkForUpdatesFromAutoCheck()
        }
    }

    private func autoUpdateChecksEnabled() -> Bool {
        if UserDefaults.standard.object(forKey: autoUpdateCheckKey) == nil {
            return true
        }
        return UserDefaults.standard.bool(forKey: autoUpdateCheckKey)
    }

    private func checkForUpdates(interactive: Bool) async {
        do {
            let release = try await fetchLatestRelease()
            let currentVersion = appVersion()
            guard compareVersions(release.version, currentVersion) == .orderedDescending else {
                if interactive {
                    showAlert(title: "已经是最新版本", message: "当前版本 \(currentVersion)，最新版本 \(release.version)。")
                }
                return
            }
            guard let asset = release.assets.first(where: { $0.name == expectedDMGAssetName(version: release.version) }) else {
                showAlert(title: "发现新版本 \(release.version)", message: "没有找到适合当前架构的 DMG。请打开 Release 页面手动下载。")
                NSWorkspace.shared.open(release.htmlURL)
                return
            }
            if confirm(title: "发现新版本 \(release.version)", message: "当前版本 \(currentVersion)。是否下载并打开适合当前 Mac 的 DMG？") {
                let dmgURL = try await downloadUpdate(asset: asset, version: release.version)
                NSWorkspace.shared.open(dmgURL)
            }
        } catch {
            if interactive {
                showAlert(title: "检查更新失败", message: String(describing: error))
            }
        }
    }

    private func fetchLatestRelease() async throws -> GitHubRelease {
        var request = URLRequest(url: URL(string: "https://api.github.com/repos/Edward-lyz/codex-mixin/releases/latest")!)
        request.setValue("Codex Mixin", forHTTPHeaderField: "User-Agent")
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let httpResponse = response as? HTTPURLResponse, httpResponse.statusCode == 200 else {
            throw GatewayError.command("GitHub release API returned a non-200 response")
        }
        return try JSONDecoder().decode(GitHubRelease.self, from: data)
    }

    private func downloadUpdate(asset: GitHubRelease.Asset, version: String) async throws -> URL {
        let downloads = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first ?? FileManager.default.homeDirectoryForCurrentUser
        let destination = downloads.appendingPathComponent(asset.name)
        if FileManager.default.fileExists(atPath: destination.path) {
            try FileManager.default.removeItem(at: destination)
        }
        var request = URLRequest(url: asset.browserDownloadURL)
        request.setValue("Codex Mixin", forHTTPHeaderField: "User-Agent")
        let (temporaryURL, response) = try await URLSession.shared.download(for: request)
        guard let httpResponse = response as? HTTPURLResponse, httpResponse.statusCode == 200 else {
            throw GatewayError.command("download failed for \(asset.name)")
        }
        try FileManager.default.moveItem(at: temporaryURL, to: destination)
        return destination
    }

    private func appVersion() -> String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.0.0"
    }

    private func expectedDMGAssetName(version: String) -> String {
        "codex-mixin-\(version)-\(macTargetTriple()).dmg"
    }

    private func macTargetTriple() -> String {
        var systemInfo = utsname()
        uname(&systemInfo)
        let machine = withUnsafePointer(to: &systemInfo.machine) {
            $0.withMemoryRebound(to: CChar.self, capacity: 1) {
                String(cString: $0)
            }
        }
        return machine == "arm64" ? "aarch64-apple-darwin" : "x86_64-apple-darwin"
    }

    @objc private func openLogs() {
        let logURL = stateDir().appendingPathComponent("gateway.log")
        if !FileManager.default.fileExists(atPath: logURL.path) {
            showAlert(title: "日志还不存在", message: "本地网关启动后会写入 \(logURL.path)。")
            return
        }
        NSWorkspace.shared.open(logURL)
    }

    @objc private func openConfigFolder() {
        do {
            try FileManager.default.createDirectory(at: stateDir(), withIntermediateDirectories: true)
            NSWorkspace.shared.open(stateDir())
        } catch {
            showAlert(title: "打开配置目录失败", message: String(describing: error))
        }
    }

    @objc private func quit() {
        timer?.invalidate()
        NSApp.terminate(nil)
    }

    private func installLaunchAgent() throws {
        try FileManager.default.createDirectory(at: stateDir(), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: launchAgentPath().deletingLastPathComponent(), withIntermediateDirectories: true)

        let executable = try gatewayExecutableURL()
        let logFile = stateDir().appendingPathComponent("gateway.log").path
        let plist = """
        <?xml version="1.0" encoding="UTF-8"?>
        <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
        <plist version="1.0">
        <dict>
          <key>Label</key>
          <string>\(serviceLabel)</string>
          <key>ProgramArguments</key>
          <array>
            <string>\(xmlEscape(executable.path))</string>
            <string>start</string>
          </array>
          <key>RunAtLoad</key>
          <true/>
          <key>StandardOutPath</key>
          <string>\(xmlEscape(logFile))</string>
          <key>StandardErrorPath</key>
          <string>\(xmlEscape(logFile))</string>
          <key>WorkingDirectory</key>
          <string>\(xmlEscape(FileManager.default.homeDirectoryForCurrentUser.path))</string>
        </dict>
        </plist>
        """
        try plist.write(to: launchAgentPath(), atomically: true, encoding: .utf8)
    }

    private func runGateway(_ arguments: [String]) async throws -> String {
        try await runProcess(try gatewayExecutableURL().path, arguments)
    }

    private func bootoutIfLoaded() async throws {
        do {
            _ = try await runProcess("/bin/launchctl", ["bootout", launchDomainAndLabel()])
        } catch {
            let message = String(describing: error)
            if !message.contains("No such process") && !message.contains("Could not find service") {
                throw error
            }
        }
    }

    private func runProcess(_ executable: String, _ arguments: [String]) async throws -> String {
        try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                let process = Process()
                let outputPipe = Pipe()
                process.executableURL = URL(fileURLWithPath: executable)
                process.arguments = arguments
                process.standardOutput = outputPipe
                process.standardError = outputPipe
                process.environment = ProcessInfo.processInfo.environment
                do {
                    try process.run()
                    process.waitUntilExit()
                    let data = outputPipe.fileHandleForReading.readDataToEndOfFile()
                    let output = String(data: data, encoding: .utf8) ?? ""
                    let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
                    if process.terminationStatus == 0 {
                        continuation.resume(returning: trimmed)
                    } else {
                        continuation.resume(throwing: GatewayError.command(trimmed.isEmpty ? "exit \(process.terminationStatus)" : trimmed))
                    }
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    private func gatewayExecutableURL() throws -> URL {
        if let resourceURL = Bundle.main.resourceURL {
            let bundled = resourceURL.appendingPathComponent("codex-mixin")
            if FileManager.default.isExecutableFile(atPath: bundled.path) {
                return bundled
            }
        }
        throw GatewayError.command("bundled codex-mixin executable not found")
    }

    private func stateDir() -> URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".codex-mixin")
    }

    private func launchAgentPath() -> URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/LaunchAgents")
            .appendingPathComponent("\(serviceLabel).plist")
    }

    private func launchDomain() -> String {
        "gui/\(getuid())"
    }

    private func launchDomainAndLabel() -> String {
        "\(launchDomain())/\(serviceLabel)"
    }
}

private struct GitHubRelease: Decodable {
    let tagName: String
    let htmlURL: URL
    let assets: [Asset]

    var version: String {
        tagName.hasPrefix("v") ? String(tagName.dropFirst()) : tagName
    }

    enum CodingKeys: String, CodingKey {
        case tagName = "tag_name"
        case htmlURL = "html_url"
        case assets
    }

    struct Asset: Decodable {
        let name: String
        let browserDownloadURL: URL

        enum CodingKeys: String, CodingKey {
            case name
            case browserDownloadURL = "browser_download_url"
        }
    }
}

private struct LoginFormValues {
    let provider: String
    let apiKey: String
    let baseUrl: String
    let gatewayKey: String
    let quotaUrl: String
    let quotaUsername: String
}

private struct QuotaUsage {
    let used: Double
    let limit: Double?
    let remaining: Double?
}

private final class ModalActionTarget: NSObject {
    private let action: () -> Void

    init(_ action: @escaping () -> Void) {
        self.action = action
    }

    @objc func run(_ sender: Any?) {
        action()
    }
}

private func runLoginSettingsPanel(stored: [String: Any]) -> LoginFormValues? {
    let providerPopup = NSPopUpButton()
    let providers: [(title: String, id: String)] = [
        ("Custom", "custom"),
        ("Baidu OneAPI", "baidu-oneapi"),
        ("OpenRouter", "openrouter"),
        ("DeepSeek", "deepseek"),
    ]
    for provider in providers {
        providerPopup.addItem(withTitle: provider.title)
        providerPopup.lastItem?.representedObject = provider.id
    }
    let storedProvider = stored["provider_preset"] as? String ?? "custom"
    if let index = providers.firstIndex(where: { $0.id == storedProvider }) {
        providerPopup.selectItem(at: index)
    }
    providerPopup.translatesAutoresizingMaskIntoConstraints = false
    providerPopup.heightAnchor.constraint(equalToConstant: 28).isActive = true

    let apiKeyField = formTextField()
    apiKeyField.placeholderString = stored["upstream_api_key"] == nil ? "必填：上游服务 API Key" : "留空保留已保存密钥"
    let baseUrlField = formTextField()
    baseUrlField.placeholderString = "只填根地址，如 https://host；不要填 /v1/messages、/v1/chat/completions 或 /anthropic"
    baseUrlField.stringValue = stored["upstream_base_url"] as? String ?? defaultBaseURL(for: storedProvider)
    let gatewayKeyField = formTextField()
    gatewayKeyField.placeholderString = stored["gateway_api_key"] == nil ? "可选：保护本地 127.0.0.1 网关的访问密钥" : "留空保留已保存密钥"
    let quotaUrlField = formTextField()
    quotaUrlField.placeholderString = "可选：完整额度查询 URL；返回 JSON 中包含 used/limit 等字段"
    quotaUrlField.stringValue = stored["quota_url"] as? String ?? defaultQuotaURL(for: storedProvider, baseURL: baseUrlField.stringValue)
    let quotaUsernameField = formTextField()
    quotaUsernameField.placeholderString = "可选：需要 username 查询参数的接口才填写"
    quotaUsernameField.stringValue = stored["quota_username"] as? String ?? ""

    let providerTarget = ModalActionTarget {
        let provider = selectedProviderID(providerPopup)
        let presetBaseURL = defaultBaseURL(for: provider)
        if !presetBaseURL.isEmpty {
            baseUrlField.stringValue = presetBaseURL
        }
        let presetQuotaURL = defaultQuotaURL(for: provider, baseURL: baseUrlField.stringValue)
        if !presetQuotaURL.isEmpty {
            quotaUrlField.stringValue = presetQuotaURL
        } else if provider != storedProvider {
            quotaUrlField.stringValue = ""
        }
    }
    providerPopup.target = providerTarget
    providerPopup.action = #selector(ModalActionTarget.run(_:))

    let contentView = NSView(frame: NSRect(x: 0, y: 0, width: 760, height: 440))
    let panel = NSPanel(
        contentRect: contentView.frame,
        styleMask: [.titled, .closable],
        backing: .buffered,
        defer: false
    )
    panel.title = "设置 Codex Mixin"
    panel.contentView = contentView
    panel.isReleasedWhenClosed = false
    panel.center()

    let titleLabel = NSTextField(labelWithString: "设置供应商与密钥")
    titleLabel.font = .boldSystemFont(ofSize: 18)
    titleLabel.textColor = .labelColor

    let detailLabel = NSTextField(wrappingLabelWithString: "配置会保存到 ~/.codex-mixin/config.json。API 密钥留空会保留已有配置；上游地址只填写根地址，路径由供应商预设或网关补齐。")
    detailLabel.textColor = .secondaryLabelColor
    detailLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    detailLabel.translatesAutoresizingMaskIntoConstraints = false
    detailLabel.widthAnchor.constraint(equalToConstant: 660).isActive = true

    let tokenButton = NSButton(title: "打开密钥页面", target: nil, action: nil)
    tokenButton.bezelStyle = .inline
    tokenButton.image = menuItemImage("key")
    tokenButton.imagePosition = .imageLeading
    tokenButton.contentTintColor = .controlAccentColor
    tokenButton.setButtonType(.momentaryPushIn)
    tokenButton.translatesAutoresizingMaskIntoConstraints = false
    let tokenTarget = ModalActionTarget {
        let provider = selectedProviderID(providerPopup)
        let rawURL = defaultCredentialURL(for: provider, baseURL: baseUrlField.stringValue)
        guard let url = URL(string: rawURL), !rawURL.isEmpty else {
            showAlert(title: "缺少密钥页面", message: "Custom 模式没有内置密钥页面。可以填写上游根地址后再打开，或直接从服务商控制台复制 API Key。")
            return
        }
        NSWorkspace.shared.open(url)
    }
    tokenButton.target = tokenTarget
    tokenButton.action = #selector(ModalActionTarget.run(_:))

    let formStack = NSStackView(views: [
        labeledView("供应商", providerPopup),
        labeledView("API 密钥", apiKeyField),
        labeledView("上游地址", baseUrlField),
        labeledView("本地密钥", gatewayKeyField),
        labeledView("额度接口", quotaUrlField),
        labeledView("额度用户", quotaUsernameField),
    ])
    formStack.orientation = .vertical
    formStack.spacing = 10

    let cancelButton = NSButton(title: "取消", target: nil, action: nil)
    cancelButton.bezelStyle = .rounded
    cancelButton.translatesAutoresizingMaskIntoConstraints = false
    cancelButton.widthAnchor.constraint(equalToConstant: 96).isActive = true
    let saveButton = NSButton(title: "保存", target: nil, action: nil)
    saveButton.bezelStyle = .rounded
    saveButton.keyEquivalent = "\r"
    saveButton.translatesAutoresizingMaskIntoConstraints = false
    saveButton.widthAnchor.constraint(equalToConstant: 96).isActive = true
    let buttonRow = NSStackView(views: [cancelButton, saveButton])
    buttonRow.orientation = .horizontal
    buttonRow.alignment = .centerY
    buttonRow.spacing = 12
    buttonRow.translatesAutoresizingMaskIntoConstraints = false

    let buttonRowContainer = NSView()
    buttonRowContainer.translatesAutoresizingMaskIntoConstraints = false
    buttonRowContainer.addSubview(buttonRow)
    NSLayoutConstraint.activate([
        buttonRowContainer.widthAnchor.constraint(equalToConstant: 660),
        buttonRowContainer.heightAnchor.constraint(equalToConstant: 34),
        buttonRow.trailingAnchor.constraint(equalTo: buttonRowContainer.trailingAnchor),
        buttonRow.centerYAnchor.constraint(equalTo: buttonRowContainer.centerYAnchor),
    ])

    let mainStack = NSStackView(views: [titleLabel, detailLabel, tokenButton, formStack, buttonRowContainer])
    mainStack.orientation = .vertical
    mainStack.alignment = .leading
    mainStack.spacing = 16
    mainStack.translatesAutoresizingMaskIntoConstraints = false
    contentView.addSubview(mainStack)
    NSLayoutConstraint.activate([
        mainStack.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 32),
        mainStack.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -32),
        mainStack.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 28),
        buttonRow.trailingAnchor.constraint(equalTo: mainStack.trailingAnchor),
    ])

    var values: LoginFormValues?
    let saveTarget = ModalActionTarget {
        values = LoginFormValues(
            provider: selectedProviderID(providerPopup),
            apiKey: apiKeyField.stringValue,
            baseUrl: baseUrlField.stringValue,
            gatewayKey: gatewayKeyField.stringValue,
            quotaUrl: quotaUrlField.stringValue,
            quotaUsername: quotaUsernameField.stringValue
        )
        NSApp.stopModal(withCode: .OK)
    }
    let cancelTarget = ModalActionTarget {
        NSApp.stopModal(withCode: .cancel)
    }
    saveButton.target = saveTarget
    saveButton.action = #selector(ModalActionTarget.run(_:))
    cancelButton.target = cancelTarget
    cancelButton.action = #selector(ModalActionTarget.run(_:))
    panel.standardWindowButton(.closeButton)?.target = cancelTarget
    panel.standardWindowButton(.closeButton)?.action = #selector(ModalActionTarget.run(_:))

    NSApp.activate(ignoringOtherApps: true)
    let response = NSApp.runModal(for: panel)
    panel.close()
    _ = tokenTarget
    _ = providerTarget
    _ = saveTarget
    _ = cancelTarget
    return response == .OK ? values : nil
}

private func quotaMenuView(title: String, detail: String?, progress: Double?) -> NSView {
    let view = NSView(frame: NSRect(x: 0, y: 0, width: 300, height: detail == nil ? 34 : 48))
    let icon = NSImageView(image: menuItemImage("chart.bar") ?? NSImage())
    icon.translatesAutoresizingMaskIntoConstraints = false
    icon.widthAnchor.constraint(equalToConstant: 18).isActive = true
    icon.heightAnchor.constraint(equalToConstant: 18).isActive = true

    let titleLabel = NSTextField(labelWithString: title)
    titleLabel.font = .systemFont(ofSize: NSFont.systemFontSize)
    titleLabel.lineBreakMode = .byTruncatingTail
    titleLabel.translatesAutoresizingMaskIntoConstraints = false

    let progressBar = NSProgressIndicator()
    progressBar.isIndeterminate = progress == nil
    progressBar.style = .bar
    progressBar.minValue = 0
    progressBar.maxValue = 1
    progressBar.doubleValue = min(max(progress ?? 0, 0), 1)
    progressBar.translatesAutoresizingMaskIntoConstraints = false
    progressBar.heightAnchor.constraint(equalToConstant: 8).isActive = true

    let rows: [NSView]
    if let detail {
        let detailLabel = NSTextField(labelWithString: detail)
        detailLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        detailLabel.textColor = .secondaryLabelColor
        rows = [titleLabel, progressBar, detailLabel]
    } else {
        rows = [titleLabel, progressBar]
    }
    let textStack = NSStackView(views: rows)
    textStack.orientation = .vertical
    textStack.alignment = .leading
    textStack.spacing = 3
    textStack.translatesAutoresizingMaskIntoConstraints = false

    view.addSubview(icon)
    view.addSubview(textStack)
    NSLayoutConstraint.activate([
        icon.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 9),
        icon.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        textStack.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 8),
        textStack.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -10),
        textStack.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        titleLabel.widthAnchor.constraint(equalTo: textStack.widthAnchor),
        progressBar.widthAnchor.constraint(equalTo: textStack.widthAnchor),
    ])
    return view
}

private func runInstallCodexPanel() -> Bool {
    let contentView = NSView(frame: NSRect(x: 0, y: 0, width: 760, height: 330))
    let panel = NSPanel(
        contentRect: contentView.frame,
        styleMask: [.titled, .closable],
        backing: .buffered,
        defer: false
    )
    panel.title = "安装到 Codex"
    panel.contentView = contentView
    panel.isReleasedWhenClosed = false
    panel.center()

    let titleLabel = NSTextField(labelWithString: "安装 Codex OAuth 代理")
    titleLabel.font = .boldSystemFont(ofSize: 18)
    titleLabel.textColor = .labelColor

    let detailLabel = NSTextField(wrappingLabelWithString: "会先备份当前 ~/.codex/config.toml，再写入独立模型目录，并把当前默认 provider 的 base_url 指向本地网关。官方 GPT 保留原名并走 Codex 官方路径，自定义 GPT 使用 -custom 后缀。完成后需要重启 Codex App；CLI 需要开新会话。")
    detailLabel.textColor = .secondaryLabelColor
    detailLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    detailLabel.translatesAutoresizingMaskIntoConstraints = false
    detailLabel.widthAnchor.constraint(equalToConstant: 660).isActive = true

    let pathStack = NSStackView(views: [
        labeledView("Codex 配置", copyableTextField("~/.codex/config.toml")),
        labeledView("模型目录", copyableTextField("~/.codex/model-catalogs/mixin-models.json")),
        labeledView("Provider", copyableTextField("保留当前 provider / requires_openai_auth")),
    ])
    pathStack.orientation = .vertical
    pathStack.spacing = 10

    let cancelButton = NSButton(title: "取消", target: nil, action: nil)
    cancelButton.bezelStyle = .rounded
    cancelButton.translatesAutoresizingMaskIntoConstraints = false
    cancelButton.widthAnchor.constraint(equalToConstant: 96).isActive = true
    let installButton = NSButton(title: "安装", target: nil, action: nil)
    installButton.bezelStyle = .rounded
    installButton.keyEquivalent = "\r"
    installButton.translatesAutoresizingMaskIntoConstraints = false
    installButton.widthAnchor.constraint(equalToConstant: 96).isActive = true

    let buttonRow = NSStackView(views: [cancelButton, installButton])
    buttonRow.orientation = .horizontal
    buttonRow.alignment = .centerY
    buttonRow.spacing = 12
    buttonRow.translatesAutoresizingMaskIntoConstraints = false

    let buttonRowContainer = NSView()
    buttonRowContainer.translatesAutoresizingMaskIntoConstraints = false
    buttonRowContainer.addSubview(buttonRow)
    NSLayoutConstraint.activate([
        buttonRowContainer.widthAnchor.constraint(equalToConstant: 660),
        buttonRowContainer.heightAnchor.constraint(equalToConstant: 34),
        buttonRow.trailingAnchor.constraint(equalTo: buttonRowContainer.trailingAnchor),
        buttonRow.centerYAnchor.constraint(equalTo: buttonRowContainer.centerYAnchor),
    ])

    let mainStack = NSStackView(views: [titleLabel, detailLabel, pathStack, buttonRowContainer])
    mainStack.orientation = .vertical
    mainStack.alignment = .leading
    mainStack.spacing = 16
    mainStack.translatesAutoresizingMaskIntoConstraints = false
    contentView.addSubview(mainStack)
    NSLayoutConstraint.activate([
        mainStack.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 32),
        mainStack.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -32),
        mainStack.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 28),
    ])

    var confirmed = false
    let installTarget = ModalActionTarget {
        confirmed = true
        NSApp.stopModal(withCode: .OK)
    }
    let cancelTarget = ModalActionTarget {
        NSApp.stopModal(withCode: .cancel)
    }
    installButton.target = installTarget
    installButton.action = #selector(ModalActionTarget.run(_:))
    cancelButton.target = cancelTarget
    cancelButton.action = #selector(ModalActionTarget.run(_:))
    panel.standardWindowButton(.closeButton)?.target = cancelTarget
    panel.standardWindowButton(.closeButton)?.action = #selector(ModalActionTarget.run(_:))

    NSApp.activate(ignoringOtherApps: true)
    let response = NSApp.runModal(for: panel)
    panel.close()
    _ = installTarget
    _ = cancelTarget
    return response == .OK && confirmed
}

private func parseQuotaUsage(_ rawJson: String) throws -> QuotaUsage {
    let data = Data(rawJson.utf8)
    guard let root = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        throw GatewayError.command("额度接口返回的 JSON 不是对象")
    }
    for payload in quotaPayloads(root) {
        guard let used = firstNumericValue(payload, ["used", "used_quota", "usage", "total_usage", "spent", "cost", "consumed"]) else {
            continue
        }
        let limit = firstNumericValue(payload, ["limit", "total", "total_credits", "quota", "quota_limit", "month_quota_limit", "budget"])
        let remaining = firstNumericValue(payload, ["remaining", "remaining_quota", "available"])
        return QuotaUsage(used: used, limit: limit, remaining: remaining)
    }
    throw GatewayError.command("额度接口返回缺少 used 字段")
}

private func quotaPayloads(_ root: [String: Any]) -> [[String: Any]] {
    var payloads = [root]
    if let data = root["data"] as? [String: Any] {
        payloads.append(data)
        if let quota = data["quota"] as? [String: Any] {
            payloads.append(quota)
        }
        if let usage = data["usage"] as? [String: Any] {
            payloads.append(usage)
        }
    }
    if let quota = root["quota"] as? [String: Any] {
        payloads.append(quota)
    }
    if let usage = root["usage"] as? [String: Any] {
        payloads.append(usage)
    }
    return payloads
}

private func firstNumericValue(_ payload: [String: Any], _ keys: [String]) -> Double? {
    for key in keys {
        if let value = numericValue(payload[key]) {
            return value
        }
    }
    return nil
}

private func numericValue(_ value: Any?) -> Double? {
    if let number = value as? NSNumber {
        return number.doubleValue
    }
    if let string = value as? String {
        return Double(string)
    }
    return nil
}

private func selectedProviderID(_ popup: NSPopUpButton) -> String {
    popup.selectedItem?.representedObject as? String ?? "custom"
}

private func defaultBaseURL(for provider: String) -> String {
    switch provider {
    case "baidu-oneapi":
        return "https://oneapi-comate.baidu-int.com"
    case "openrouter":
        return "https://openrouter.ai/api"
    case "deepseek":
        return "https://api.deepseek.com"
    default:
        return ""
    }
}

private func defaultQuotaURL(for provider: String, baseURL: String) -> String {
    let trimmed = baseURL.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    switch provider {
    case "baidu-oneapi":
        return trimmed.isEmpty ? "" : "\(trimmed)/openapi/v3/user/quota"
    case "openrouter":
        return trimmed.isEmpty ? "" : "\(trimmed)/v1/credits"
    default:
        return ""
    }
}

private func defaultCredentialURL(for provider: String, baseURL: String) -> String {
    switch provider {
    case "baidu-oneapi":
        return "https://oneapi-comate.baidu-int.com/token"
    case "openrouter":
        return "https://openrouter.ai/settings/keys"
    case "deepseek":
        return "https://platform.deepseek.com/api_keys"
    default:
        return baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

private func compareVersions(_ lhs: String, _ rhs: String) -> ComparisonResult {
    let leftParts = lhs.split(separator: ".").map { Int($0) ?? 0 }
    let rightParts = rhs.split(separator: ".").map { Int($0) ?? 0 }
    let count = max(leftParts.count, rightParts.count)
    for index in 0..<count {
        let left = index < leftParts.count ? leftParts[index] : 0
        let right = index < rightParts.count ? rightParts[index] : 0
        if left < right {
            return .orderedAscending
        }
        if left > right {
            return .orderedDescending
        }
    }
    return .orderedSame
}

private func formatQuotaAmount(_ value: Double) -> String {
    let formatter = NumberFormatter()
    formatter.minimumFractionDigits = value.rounded() == value ? 0 : 2
    formatter.maximumFractionDigits = 2
    return formatter.string(from: NSNumber(value: value)) ?? String(format: "%.2f", value)
}

private func menuItemImage(_ systemSymbolName: String) -> NSImage? {
    guard #available(macOS 11.0, *) else {
        return nil
    }
    guard let image = NSImage(systemSymbolName: systemSymbolName, accessibilityDescription: nil) else {
        return nil
    }
    image.isTemplate = true
    return image
}

private func codexStatusImage(isRunning: Bool) -> NSImage {
    let size = NSSize(width: 22, height: 22)
    let image = NSImage(size: size)
    image.lockFocus()

    let bounds = NSRect(origin: .zero, size: size)
    NSColor.clear.setFill()
    bounds.fill()

    let shadow = NSShadow()
    shadow.shadowOffset = NSSize(width: 0, height: -0.6)
    shadow.shadowBlurRadius = 1.6
    shadow.shadowColor = NSColor.black.withAlphaComponent(0.22)
    shadow.set()

    let body = NSBezierPath(roundedRect: NSRect(x: 2.2, y: 2.0, width: 17.8, height: 17.8), xRadius: 6.0, yRadius: 6.0)
    let startColor = NSColor(calibratedRed: 0.20, green: 0.53, blue: 1.00, alpha: 1.0)
    let endColor = NSColor(calibratedRed: 0.54, green: 0.32, blue: 0.98, alpha: 1.0)
    NSGradient(starting: startColor, ending: endColor)?.draw(in: body, angle: 35)

    let glow = NSBezierPath(ovalIn: NSRect(x: 3.7, y: 9.8, width: 15.2, height: 8.0))
    NSColor.white.withAlphaComponent(0.20).setFill()
    glow.fill()

    let prompt = NSBezierPath()
    prompt.lineWidth = 1.9
    prompt.lineCapStyle = .round
    prompt.lineJoinStyle = .round
    prompt.move(to: NSPoint(x: 7.2, y: 8.0))
    prompt.line(to: NSPoint(x: 10.2, y: 11.0))
    prompt.line(to: NSPoint(x: 7.2, y: 14.0))
    NSColor.white.withAlphaComponent(0.95).setStroke()
    prompt.stroke()

    let cursor = NSBezierPath()
    cursor.lineWidth = 1.9
    cursor.lineCapStyle = .round
    cursor.move(to: NSPoint(x: 12.4, y: 8.2))
    cursor.line(to: NSPoint(x: 15.8, y: 8.2))
    cursor.stroke()

    let statusDot = NSBezierPath(ovalIn: NSRect(x: 15.7, y: 3.2, width: 4.2, height: 4.2))
    (isRunning ? NSColor.systemGreen : NSColor.systemOrange).setFill()
    statusDot.fill()

    image.unlockFocus()
    image.isTemplate = false
    return image
}

private enum GatewayError: Error, CustomStringConvertible {
    case command(String)

    var description: String {
        switch self {
        case .command(let message):
            return message
        }
    }
}

private func labeledView(_ title: String, _ field: NSView) -> NSView {
    let label = NSTextField(labelWithString: title)
    label.alignment = .right
    label.textColor = .secondaryLabelColor
    label.translatesAutoresizingMaskIntoConstraints = false
    label.widthAnchor.constraint(equalToConstant: 110).isActive = true
    field.translatesAutoresizingMaskIntoConstraints = false
    field.widthAnchor.constraint(equalToConstant: 540).isActive = true
    let row = NSStackView(views: [label, field])
    row.orientation = .horizontal
    row.alignment = .centerY
    row.spacing = 10
    return row
}

private func formTextField() -> NSTextField {
    let field = NSTextField()
    field.controlSize = .regular
    field.font = .systemFont(ofSize: NSFont.systemFontSize)
    field.lineBreakMode = .byTruncatingMiddle
    field.translatesAutoresizingMaskIntoConstraints = false
    field.heightAnchor.constraint(equalToConstant: 28).isActive = true
    return field
}

private func copyableTextField(_ value: String) -> NSTextField {
    let field = NSTextField()
    field.stringValue = value
    field.isEditable = false
    field.isSelectable = true
    field.isBordered = false
    field.drawsBackground = false
    field.font = .systemFont(ofSize: NSFont.systemFontSize)
    field.textColor = .labelColor
    field.lineBreakMode = .byTruncatingMiddle
    field.translatesAutoresizingMaskIntoConstraints = false
    field.heightAnchor.constraint(equalToConstant: 28).isActive = true
    return field
}

private func loadStoredConfig() throws -> [String: Any] {
    let configURL = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".codex-mixin")
        .appendingPathComponent("config.json")
    guard FileManager.default.fileExists(atPath: configURL.path) else {
        return [:]
    }
    let data = try Data(contentsOf: configURL)
    guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        throw GatewayError.command("\(configURL.path) 不是 JSON 对象")
    }
    return object
}

private func appendOptionalArg(_ args: inout [String], _ name: String, _ rawValue: String) {
    let value = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
    if !value.isEmpty {
        args.append(name)
        args.append(value)
    }
}

private func showAlert(title: String, message: String) {
    if !Thread.isMainThread {
        DispatchQueue.main.sync {
            showAlert(title: title, message: message)
        }
        return
    }
    let alert = NSAlert()
    alert.messageText = title
    alert.informativeText = message
    alert.alertStyle = title.contains("失败") || title.contains("缺少") || title.contains("错误") ? .warning : .informational
    alert.addButton(withTitle: "确定")
    NSApp.activate(ignoringOtherApps: true)
    alert.runModal()
}

private func confirm(title: String, message: String) -> Bool {
    if !Thread.isMainThread {
        return DispatchQueue.main.sync {
            confirm(title: title, message: message)
        }
    }
    let alert = NSAlert()
    alert.messageText = title
    alert.informativeText = message
    alert.alertStyle = .warning
    alert.addButton(withTitle: "继续")
    alert.addButton(withTitle: "取消")
    NSApp.activate(ignoringOtherApps: true)
    return alert.runModal() == .alertFirstButtonReturn
}

private func xmlEscape(_ value: String) -> String {
    value
        .replacingOccurrences(of: "&", with: "&amp;")
        .replacingOccurrences(of: "\"", with: "&quot;")
        .replacingOccurrences(of: "'", with: "&apos;")
        .replacingOccurrences(of: "<", with: "&lt;")
        .replacingOccurrences(of: ">", with: "&gt;")
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.run()
