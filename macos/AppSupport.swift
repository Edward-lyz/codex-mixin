import Cocoa
import SwiftUI

enum GatewayError: Error, CustomStringConvertible {
    case command(String)

    var description: String {
        switch self {
        case .command(let message):
            return message
        }
    }
}

func localizedPrompt(_ text: String) -> String {
    let translations: [String: (traditional: String, english: String)] = [
        "启动服务失败": ("啟動服務失敗", "Unable to Start Service"),
        "重启服务失败": ("重新啟動服務失敗", "Unable to Restart Service"),
        "自动启动网关失败": ("自動啟動閘道失敗", "Unable to Start Gateway Automatically"),
        "刷新 Codex 模型失败": ("重新整理 Codex 模型失敗", "Unable to Refresh Codex Models"),
        "更新登录自启失败": ("更新登入時啟動失敗", "Unable to Update Launch at Login"),
        "健康检测和修复失败": ("健康檢測與修復失敗", "Health Check & Repair Failed"),
        "打开配置目录失败": ("開啟設定目錄失敗", "Unable to Open Configuration Folder"),
        "日志还不存在": ("日誌尚不存在", "No Log File Yet"),
        "退出 Codex Mixin 失败": ("結束 Codex Mixin 失敗", "Unable to Quit Codex Mixin"),
        "安装到 Codex 失败": ("安裝到 Codex 失敗", "Unable to Install into Codex"),
        "从 Codex 恢复失败": ("從 Codex 還原失敗", "Unable to Restore Codex"),
        "Codex 配置已恢复": ("Codex 設定已還原", "Codex Configuration Restored"),
        "安装到 Claude Code 失败": ("安裝到 Claude Code 失敗", "Unable to Install into Claude Code"),
        "从 Claude Code 恢复失败": ("從 Claude Code 還原失敗", "Unable to Restore Claude Code"),
        "Claude Code 配置已恢复": ("Claude Code 設定已還原", "Claude Code Configuration Restored"),
        "复制本地接口失败": ("複製本機端點失敗", "Unable to Copy Local Endpoint"),
        "读取供应商失败": ("讀取供應商失敗", "Unable to Read Providers"),
        "供应商操作失败": ("供應商操作失敗", "Provider Operation Failed"),
        "连接测试失败": ("連線測試失敗", "Connection Test Failed"),
        "连接测试通过": ("連線測試通過", "Connection Test Passed"),
        "启动测速失败": ("啟動測速失敗", "Unable to Start Benchmark"),
        "保存 Fusion 设置失败": ("儲存 Fusion 設定失敗", "Unable to Save Fusion Settings"),
        "缺少 API 密钥": ("缺少 API 金鑰", "API Key Required"),
        "缺少额度用户名": ("缺少額度使用者名稱", "Quota Username Required"),
        "缺少 OpenCode Go 额度凭据": (
            "缺少 OpenCode Go 額度憑證",
            "OpenCode Go Quota Credentials Required"
        ),
        "缺少 Provider ID": ("缺少 Provider ID", "Provider ID Required"),
        "缺少显示名称": ("缺少顯示名稱", "Display Name Required"),
        "缺少密钥页面": ("缺少金鑰頁面", "No API Key Page"),
    ]
    guard let translation = translations[text] else { return text }
    switch UpdateLanguage.current {
    case .simplifiedChinese, .traditionalChinese: return text
    case .english: return translation.english
    }
}

func localizedErrorDescription(_ error: Error) -> String {
    localizedGatewayMessage(String(describing: error))
}

func localizedGatewayMessage(_ rawMessage: String) -> String {
    let message = rawMessage.hasPrefix("Error: ")
        ? String(rawMessage.dropFirst("Error: ".count))
        : rawMessage
    guard UpdateLanguage.current != .english else { return message }
    let replacements: [(String, String, String)] = [
        (
            "provider configuration is missing",
            "尚未配置供应商",
            "尚未設定供應商"
        ),
        (
            "provider configuration is empty",
            "供应商配置为空",
            "供應商設定為空"
        ),
        (
            "configuration has no config_version and does not match the legacy single-provider format",
            "配置文件既没有版本号，也不符合旧版单供应商格式",
            "設定檔既沒有版本號，也不符合舊版單一供應商格式"
        ),
        ("unsupported config version", "不支持的配置版本", "不支援的設定版本"),
        ("gateway not running", "本地网关未运行", "本機閘道未執行"),
        ("unknown provider:", "未知供应商：", "未知供應商："),
        (
            "quota endpoint is not configured",
            "未配置额度接口",
            "未設定額度端點"
        ),
        (
            "quota response does not contain a valid used amount",
            "额度响应中没有有效的已用额度",
            "額度回應中沒有有效的已用額度"
        ),
        ("quota endpoint returned", "额度接口返回", "額度端點回傳"),
        (
            "could not parse OpenCode Go dashboard usage",
            "OpenCode Go 用量页面解析失败",
            "OpenCode Go 用量頁面解析失敗"
        ),
        (
            "could not parse OpenCode Go billing data",
            "OpenCode Go 余额页面解析失败",
            "OpenCode Go 餘額頁面解析失敗"
        ),
        ("OpenCode Go dashboard error", "OpenCode Go 页面请求失败", "OpenCode Go 頁面請求失敗"),
        ("models endpoint returned", "模型接口返回", "模型端點回傳"),
        ("available-models endpoint returned", "可用模型接口返回", "可用模型端點回傳"),
        ("error sending request for url", "请求发送失败", "要求傳送失敗"),
        ("connection error", "连接错误", "連線錯誤"),
        ("operation timed out", "操作超时", "操作逾時"),
        ("missing field", "缺少字段", "缺少欄位"),
        ("at line", "位于第", "位於第"),
        ("column", "列", "欄"),
    ]
    let replacementIndex = UpdateLanguage.current == .simplifiedChinese ? 1 : 2
    return replacements.reduce(message) { localized, replacement in
        let target = replacementIndex == 1 ? replacement.1 : replacement.2
        return localized.replacingOccurrences(of: replacement.0, with: target)
    }
}


func showAlert(title: String, message: String) {
    if !Thread.isMainThread {
        DispatchQueue.main.sync {
            showAlert(title: title, message: message)
        }
        return
    }
    let alert = NSAlert()
    let localizedTitle = localizedPrompt(title)
    alert.messageText = localizedTitle
    alert.informativeText = localizedGatewayMessage(message)
    alert.alertStyle = title.contains("失败")
        || title.contains("缺少")
        || title.contains("错误")
        || localizedTitle.contains("Failed")
        || localizedTitle.contains("Unable")
        ? .warning
        : .informational
    alert.addButton(withTitle: AppLocalization.string("appSupport.ok"))
    NSApp.activate(ignoringOtherApps: true)
    alert.runModal()
}

func confirm(title: String, message: String) -> Bool {
    if !Thread.isMainThread {
        return DispatchQueue.main.sync {
            confirm(title: title, message: message)
        }
    }
    let alert = NSAlert()
    alert.messageText = localizedPrompt(title)
    alert.informativeText = localizedGatewayMessage(message)
    alert.alertStyle = .warning
    alert.addButton(withTitle: AppLocalization.string("appSupport.continue"))
    alert.addButton(withTitle: AppLocalization.string("appSupport.cancel"))
    NSApp.activate(ignoringOtherApps: true)
    return alert.runModal() == .alertFirstButtonReturn
}

func showDiagnosticReport(title: String, report: String) {
    if !Thread.isMainThread {
        DispatchQueue.main.sync {
            showDiagnosticReport(title: title, report: report)
        }
        return
    }
    let informativeText = report.contains("[ERROR]")
        ? AppLocalization.string("appSupport.issuesWereDetectedTheReportIncludesThe")
        : AppLocalization.string("appSupport.checkCompletedCopyTheReportWhenReporting")
    let window = NSWindow(
        contentRect: NSRect(x: 0, y: 0, width: 760, height: 560),
        styleMask: [.titled, .closable, .resizable],
        backing: .buffered,
        defer: false
    )
    window.title = localizedPrompt(title)
    window.minSize = NSSize(width: 620, height: 420)
    window.center()
    let close = { NSApp.stopModal(withCode: .cancel) }
    let copy = {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(report, forType: .string)
    }
    window.contentViewController = NSHostingController(rootView: DiagnosticReportView(
        title: localizedPrompt(title),
        informativeText: informativeText,
        report: report,
        hasErrors: report.contains("[ERROR]"),
        close: close,
        copy: copy
    ))
    let closeTarget = DiagnosticModalTarget(close)
    window.standardWindowButton(.closeButton)?.target = closeTarget
    window.standardWindowButton(.closeButton)?.action = #selector(DiagnosticModalTarget.run(_:))
    NSApp.activate(ignoringOtherApps: true)
    NSApp.runModal(for: window)
    window.close()
    _ = closeTarget
}

private struct DiagnosticReportView: View {
    let title: String
    let informativeText: String
    let report: String
    let hasErrors: Bool
    let close: () -> Void
    let copy: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label(title, systemImage: hasErrors ? "exclamationmark.triangle.fill" : "checkmark.circle.fill")
                .font(.title2.weight(.semibold))
                .foregroundStyle(hasErrors ? Color.orange : Color.green)
            Text(informativeText)
                .foregroundStyle(.secondary)
            ScrollView([.horizontal, .vertical]) {
                Text(report)
                    .font(.system(size: 12, design: .monospaced))
                    .textSelection(.enabled)
                    .fixedSize(horizontal: true, vertical: true)
                    .padding(12)
            }
            .background(Color(nsColor: .textBackgroundColor))
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay {
                RoundedRectangle(cornerRadius: 8).stroke(.separator)
            }
            HStack {
                Spacer()
                Button(AppLocalization.string("appSupport.copyReport"), action: copy)
                Button(AppLocalization.string("appSupport.close"), action: close)
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.borderedProminent)
            }
        }
        .padding(24)
        .frame(minWidth: 620, minHeight: 420)
        .background(Color(nsColor: .windowBackgroundColor))
    }
}

private final class DiagnosticModalTarget: NSObject {
    let action: () -> Void

    init(_ action: @escaping () -> Void) {
        self.action = action
    }

    @objc func run(_ sender: Any?) {
        action()
    }
}

func xmlEscape(_ value: String) -> String {
    value
        .replacingOccurrences(of: "&", with: "&amp;")
        .replacingOccurrences(of: "\"", with: "&quot;")
        .replacingOccurrences(of: "'", with: "&apos;")
        .replacingOccurrences(of: "<", with: "&lt;")
        .replacingOccurrences(of: ">", with: "&gt;")
}

extension NSColor {
    static let mixinHealthy = NSColor.systemGreen
    static let mixinDegraded = NSColor.systemOrange
    static let mixinError = NSColor.systemRed
    static let mixinIdle = NSColor.systemGray
    static let mixinAccent = NSColor.controlAccentColor
}
