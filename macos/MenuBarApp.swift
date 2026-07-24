import Cocoa
import Darwin

final class AppDelegate: NSObject, NSApplicationDelegate {
    let serviceLabel = "local.codex-mixin.service"
    let menuLaunchLabel = "local.codex-mixin.menu-launch"
    var statusItem: NSStatusItem?
    var serviceStatusItem: NSMenuItem?
    var quotaStatusItem: NSMenuItem?
    var startMenuItem: NSMenuItem?
    var restartMenuItem: NSMenuItem?
    var launchAtLoginMenuItem: NSMenuItem?
    var providerSettingsWindowController: ProviderSettingsWindowController?
    var modelBenchmarkWindowController: ModelBenchmarkWindowController?
    var fusionSettingsWindowController: FusionSettingsWindowController?
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
    var serviceStatus = "本地网关检查中..." {
        didSet { updateServiceStatusView() }
    }
    var serviceEndpoint: String? {
        didSet { updateServiceStatusView() }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        installApplicationMenu()
        installStatusItem()
        startGatewayAtLaunch()
        timer = Timer.scheduledTimer(withTimeInterval: 60, repeats: true) { [weak self] _ in
            Task { @MainActor in
                self?.refreshStatus()
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
        serviceItem.isEnabled = false
        let quotaItem = NSMenuItem(title: "", action: nil, keyEquivalent: "")
        quotaItem.isEnabled = false
        quotaItem.image = menuItemImage("chart.bar")
        serviceStatusItem = serviceItem
        quotaStatusItem = quotaItem
        menu.addItem(serviceItem)
        updateServiceStatusView()
        menu.addItem(quotaItem)
        updateQuotaStatus(title: "额度：检查中...", detail: nil, progress: nil)
        menu.addItem(.separator())
        startMenuItem = actionItem("启动本地网关", #selector(startService), "play.circle")
        restartMenuItem = actionItem("重启本地网关", #selector(restartService), "arrow.clockwise.circle")
        menu.addItem(startMenuItem!)
        menu.addItem(restartMenuItem!)
        launchAtLoginMenuItem = actionItem("登录时启动并开启服务", #selector(toggleLaunchAtLogin), "poweron")
        menu.addItem(launchAtLoginMenuItem!)
        menu.addItem(actionItem("刷新状态与额度", #selector(refreshStatus), "arrow.clockwise"))
        menu.addItem(actionItem("自动检测...", #selector(runAutomaticDoctor), "stethoscope"))
        menu.addItem(.separator())
        menu.addItem(actionItem("设置供应商与密钥...", #selector(configureLogin), "gearshape"))
        menu.addItem(actionItem("Fusion 设置…", #selector(showFusionSettings), "rectangle.3.group"))
        menu.addItem(actionItem("模型测速...", #selector(showModelBenchmark), "speedometer"))
        menu.addItem(actionItem("安装到 Codex...", #selector(installCodexConfig), "square.and.arrow.down"))
        menu.addItem(actionItem("从 Codex 恢复...", #selector(uninstallCodexConfig), "arrow.uturn.backward.circle"))
        menu.addItem(.separator())
        menu.addItem(actionItem("检查更新...", #selector(checkForUpdatesFromMenu), "arrow.down.circle"))
        menu.addItem(.separator())
        menu.addItem(actionItem("复制本地接口地址", #selector(copyLocalEndpoint), "link"))
        menu.addItem(actionItem("打开运行日志", #selector(openLogs), "doc.text"))
        menu.addItem(actionItem("打开配置目录", #selector(openConfigFolder), "folder"))
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

    func updateStatusTitle() {
        statusItem?.button?.image = codexStatusImage(isRunning: isRunning)
        statusItem?.button?.toolTip = isRunning ? "Codex Mixin：运行中" : "Codex Mixin：已停止"
        updateServiceStatusView()
    }

    func updateServiceStatusView() {
        guard let serviceStatusItem else { return }
        let title = serviceStatus
        let endpoint = serviceEndpoint
        let running = isRunning
        let busy = serviceBusy
        menuItemViewUpdater.setView(for: serviceStatusItem) {
            serviceMenuView(
                title: title,
                endpoint: endpoint,
                isRunning: running,
                isBusy: busy
            )
        }
    }

    func updateActionStates() {
        startMenuItem?.isEnabled = !serviceBusy && !isRunning
        restartMenuItem?.isEnabled = !serviceBusy
        launchAtLoginMenuItem?.state = FileManager.default.fileExists(atPath: launchAgentPath().path) ? .on : .off
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
