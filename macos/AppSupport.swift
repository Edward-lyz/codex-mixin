import Cocoa

enum GatewayError: Error, CustomStringConvertible {
    case command(String)

    var description: String {
        switch self {
        case .command(let message):
            return message
        }
    }
}

func appText(_ simplifiedChinese: String, _ traditionalChinese: String, _ english: String) -> String {
    switch UpdateLanguage.current {
    case .simplifiedChinese: return simplifiedChinese
    case .traditionalChinese: return traditionalChinese
    case .english: return english
    }
}

func localizedPrompt(_ text: String) -> String {
    let translations: [String: (traditional: String, english: String)] = [
        "启动服务失败": ("啟動服務失敗", "Unable to Start Service"),
        "重启服务失败": ("重新啟動服務失敗", "Unable to Restart Service"),
        "自动启动网关失败": ("自動啟動閘道失敗", "Unable to Start Gateway Automatically"),
        "刷新 Codex 模型失败": ("重新整理 Codex 模型失敗", "Unable to Refresh Codex Models"),
        "更新登录自启失败": ("更新登入時啟動失敗", "Unable to Update Launch at Login"),
        "自动检测失败": ("自動檢測失敗", "Automatic Check Failed"),
        "打开配置目录失败": ("開啟設定目錄失敗", "Unable to Open Configuration Folder"),
        "日志还不存在": ("日誌尚不存在", "No Log File Yet"),
        "退出 Codex Mixin 失败": ("結束 Codex Mixin 失敗", "Unable to Quit Codex Mixin"),
        "安装到 Codex 失败": ("安裝到 Codex 失敗", "Unable to Install into Codex"),
        "从 Codex 恢复失败": ("從 Codex 還原失敗", "Unable to Restore Codex"),
        "Codex 配置已恢复": ("Codex 設定已還原", "Codex Configuration Restored"),
        "复制本地接口失败": ("複製本機端點失敗", "Unable to Copy Local Endpoint"),
        "读取供应商失败": ("讀取供應商失敗", "Unable to Read Providers"),
        "供应商操作失败": ("供應商操作失敗", "Provider Operation Failed"),
        "连接测试失败": ("連線測試失敗", "Connection Test Failed"),
        "连接测试通过": ("連線測試通過", "Connection Test Passed"),
        "启动测速失败": ("啟動測速失敗", "Unable to Start Benchmark"),
        "保存 Fusion 设置失败": ("儲存 Fusion 設定失敗", "Unable to Save Fusion Settings"),
        "缺少 API 密钥": ("缺少 API 金鑰", "API Key Required"),
        "缺少额度用户名": ("缺少額度使用者名稱", "Quota Username Required"),
        "缺少 Provider ID": ("缺少 Provider ID", "Provider ID Required"),
        "缺少显示名称": ("缺少顯示名稱", "Display Name Required"),
        "缺少密钥页面": ("缺少金鑰頁面", "No API Key Page"),
    ]
    guard let translation = translations[text] else { return text }
    switch UpdateLanguage.current {
    case .simplifiedChinese: return text
    case .traditionalChinese: return translation.traditional
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
    alert.addButton(withTitle: appText("确定", "確定", "OK"))
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
    alert.addButton(withTitle: appText("继续", "繼續", "Continue"))
    alert.addButton(withTitle: appText("取消", "取消", "Cancel"))
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
    let textView = NSTextView(frame: NSRect(x: 0, y: 0, width: 680, height: 420))
    textView.string = report
    textView.isEditable = false
    textView.isSelectable = true
    textView.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
    textView.textContainerInset = NSSize(width: 10, height: 10)

    let scrollView = NSScrollView(frame: textView.frame)
    scrollView.documentView = textView
    scrollView.hasVerticalScroller = true
    scrollView.hasHorizontalScroller = true
    scrollView.autohidesScrollers = true
    scrollView.borderType = .bezelBorder

    let alert = NSAlert()
    alert.messageText = localizedPrompt(title)
    alert.informativeText = report.contains("[ERROR]")
        ? appText(
            "检测到需要处理的问题。完整错误链已包含在报告中。",
            "檢測到需要處理的問題。完整錯誤鏈已包含在報告中。",
            "Issues were detected. The report includes the complete error chain."
        )
        : appText(
            "检测完成。可复制报告用于反馈问题。",
            "檢測完成。可複製報告以回報問題。",
            "Check completed. Copy the report when reporting an issue."
        )
    alert.alertStyle = report.contains("[ERROR]") ? .warning : .informational
    alert.accessoryView = scrollView
    alert.addButton(withTitle: appText("关闭", "關閉", "Close"))
    alert.addButton(withTitle: appText("复制报告", "複製報告", "Copy Report"))
    NSApp.activate(ignoringOtherApps: true)
    if alert.runModal() == .alertSecondButtonReturn {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(report, forType: .string)
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
