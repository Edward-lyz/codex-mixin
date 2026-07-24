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


func showAlert(title: String, message: String) {
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

func confirm(title: String, message: String) -> Bool {
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
    alert.messageText = title
    alert.informativeText = report.contains("[ERROR]")
        ? "检测到需要处理的问题。完整错误链已包含在报告中。"
        : "检测完成。可复制报告用于反馈问题。"
    alert.alertStyle = report.contains("[ERROR]") ? .warning : .informational
    alert.accessoryView = scrollView
    alert.addButton(withTitle: "关闭")
    alert.addButton(withTitle: "复制报告")
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
