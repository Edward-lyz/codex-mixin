import Cocoa
import Darwin

final class AppDelegate: NSObject, NSApplicationDelegate {
    let serviceLabel = "local.codex-mixin.service"
    let menuLaunchLabel = "local.codex-mixin.menu-launch"
    var statusItem: NSStatusItem?
    var serviceStatusItem: NSMenuItem?
    var providerUsageDashboardView: ProviderUsageDashboardView?
    var launchAtLoginMenuItem: NSMenuItem?
    var providerSettingsWindowController: ProviderSettingsWindowController?
    var modelBenchmarkWindowController: ModelBenchmarkWindowController?
    var fusionSettingsWindowController: FusionSettingsWindowController?
    var aboutWindowController: AboutWindowController?
    var installCardWindowController: InstallCardWindowController?
    let menuItemViewUpdater = MenuItemViewUpdater()
    var timer: Timer?
    var terminationInProgress = false
    var isRunning = false
    var serviceBusy = false {
        didSet {
            updateActionStates()
            updateServiceStatusView()
        }
    }
    var automaticDoctorBusy = false
    lazy var statusRefreshCoordinator = StatusRefreshCoordinator { [weak self] isCurrent in
        await self?.performStatusRefresh(isCurrent: isCurrent)
    }
    var pendingStatusRefreshScope: StatusRefreshScope?
    var quotaRefreshPolicy = QuotaRefreshPolicy()
    var serviceStatus = "本地网关检查中..." {
        didSet { updateServiceStatusView() }
    }
    var serviceEndpoint: String? {
        didSet { updateServiceStatusView() }
    }
    var providerStatusDetail: String? {
        didSet { updateServiceStatusView() }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        CardIdentityStore.standard.current()
        installApplicationMenu()
        installStatusItem()
        menuItemViewUpdater.onMenuWillOpen = { [weak self] in
            Task { @MainActor in
                await self?.refreshMenuStatus()
            }
        }
        startGatewayAtLaunch()
        timer = Timer.scheduledTimer(withTimeInterval: 10, repeats: true) { [weak self] _ in
            Task { @MainActor in
                await self?.refreshScheduledStatus()
            }
        }
        if !CommandLine.arguments.contains("--check-updates") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak self] in
                Task { @MainActor in
                    await self?.checkForUpdates(interactive: false)
                }
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

    func installApplicationMenu() {
        NSApp.mainMenu = makeApplicationMainMenu(
            quitTarget: self,
            quitAction: #selector(quit),
            closeWindowTarget: self,
            closeWindowAction: #selector(closeCurrentWindow)
        )
    }

    @objc func closeCurrentWindow(_ sender: Any?) {
        closeWindow(NSApp.keyWindow ?? NSApp.mainWindow, sender: sender)
    }

    func installStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.title = ""
        item.button?.toolTip = "Codex Mixin"
        item.button?.image = codexStatusImage(isRunning: false)
        item.button?.imagePosition = .imageOnly
        item.menu = buildMenu()
        statusItem = item
    }

    func buildMenu() -> NSMenu {
        let menu = NSMenu()
        menu.delegate = menuItemViewUpdater
        let serviceItem = NSMenuItem(title: serviceStatus, action: nil, keyEquivalent: "")
        let providerUsageItem = NSMenuItem(title: "", action: nil, keyEquivalent: "")
        let providerUsageView = ProviderUsageDashboardView()
        providerUsageItem.view = providerUsageView
        serviceStatusItem = serviceItem
        providerUsageDashboardView = providerUsageView
        menu.addItem(serviceItem)
        updateServiceStatusView()
        menu.addItem(providerUsageItem)
        updateQuotaStatus(title: "额度：检查中...", detail: nil, progress: nil)
        updateTokenUsageStatus(title: "Token 使用：检查中...", detail: nil, progress: nil)
        menu.addItem(.separator())
        launchAtLoginMenuItem = actionItem("登录时启动并开启服务", #selector(toggleLaunchAtLogin), "poweron")
        menu.addItem(launchAtLoginMenuItem!)
        menu.addItem(actionItem("刷新状态与额度", #selector(refreshStatus), "arrow.clockwise"))
        menu.addItem(actionItem("健康检测和修复...", #selector(runAutomaticDoctor), "stethoscope"))
        menu.addItem(.separator())
        menu.addItem(submenuItem("设置与模型", symbolName: "gearshape", items: [
            actionItem("供应商设置...", #selector(configureLogin), "gearshape"),
            actionItem("模型选择与测速...", #selector(showModelBenchmark), "speedometer"),
            actionItem("Fusion 设置…", #selector(showFusionSettings), "rectangle.3.group")
        ]))
        menu.addItem(submenuItem("安装与恢复", symbolName: "square.and.arrow.down", items: [
            actionItem("安装到 Codex...", #selector(installCodexConfig), "square.and.arrow.down"),
            actionItem("从 Codex 恢复...", #selector(uninstallCodexConfig), "arrow.uturn.backward.circle"),
            actionItem("安装到 Claude Code...", #selector(installClaudeCode), "square.and.arrow.down"),
            actionItem("从 Claude Code 恢复...", #selector(uninstallClaudeCode), "arrow.uturn.backward.circle"),
            actionItem("安装到 DSH...", #selector(installDsh), "square.and.arrow.down"),
            actionItem("从 DSH 卸载...", #selector(uninstallDsh), "arrow.uturn.backward.circle")
        ]))
        menu.addItem(submenuItem("关于", symbolName: "info.circle", items: [
            actionItem("关于 Codex Mixin...", #selector(showAbout), "info.circle"),
            actionItem("检查更新...", #selector(checkForUpdatesFromMenu), "arrow.down.circle"),
            actionItem("复制本地接口地址", #selector(copyLocalEndpoint), "link"),
            actionItem("打开运行日志", #selector(openLogs), "doc.text"),
            actionItem("打开配置目录", #selector(openConfigFolder), "folder")
        ]))
        menu.addItem(.separator())
        menu.addItem(actionItem("退出 Codex Mixin", #selector(quit), "power"))
        updateActionStates()
        return menu
    }

    func actionItem(_ title: String, _ action: Selector, _ symbolName: String) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
        item.target = self
        item.image = menuItemImage(symbolName)
        return item
    }

    func submenuItem(_ title: String, symbolName: String, items: [NSMenuItem]) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.image = menuItemImage(symbolName)
        let submenu = NSMenu(title: title)
        items.forEach(submenu.addItem)
        item.submenu = submenu
        return item
    }

    func updateStatusTitle() {
        statusItem?.button?.image = codexStatusImage(isRunning: isRunning)
        statusItem?.button?.toolTip = isRunning ? "Codex Mixin：运行中" : "Codex Mixin：已停止"
        updateServiceStatusView()
    }

    func updateServiceStatusView() {
        guard let serviceStatusItem else { return }
        let title = serviceStatus
        let endpoint = serviceEndpoint
        let statusDetail = serviceStatus.contains("降级") ? providerStatusDetail : nil
        let running = isRunning
        let busy = serviceBusy
        if let view = serviceStatusItem.view,
           updateServiceMenuView(
               view,
               title: title,
               endpoint: endpoint,
               statusDetail: statusDetail,
               isRunning: running,
               isBusy: busy
           ) {
            return
        }
        menuItemViewUpdater.setView(for: serviceStatusItem) {
            serviceMenuView(
                title: title,
                endpoint: endpoint,
                statusDetail: statusDetail,
                isRunning: running,
                isBusy: busy,
                target: self,
                action: #selector(AppDelegate.toggleGateway(_:))
            )
        }
    }

    func updateActionStates() {
        updateServiceStatusView()
        launchAtLoginMenuItem?.state = FileManager.default.fileExists(atPath: launchAgentPath().path) ? .on : .off
    }

    @objc func toggleGateway(_ sender: GatewaySwitchControl) {
        sender.isEnabled = false
        sender.isBusy = true
        if sender.isOn {
            startService()
        } else {
            stopService()
        }
    }

}

@main
struct CodexMixinApplication {
    static let delegate = AppDelegate()

    static func main() {
        let app = NSApplication.shared
        app.delegate = delegate
        app.run()
    }
}
