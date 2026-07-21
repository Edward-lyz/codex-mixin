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

func xmlEscape(_ value: String) -> String {
    value
        .replacingOccurrences(of: "&", with: "&amp;")
        .replacingOccurrences(of: "\"", with: "&quot;")
        .replacingOccurrences(of: "'", with: "&apos;")
        .replacingOccurrences(of: "<", with: "&lt;")
        .replacingOccurrences(of: ">", with: "&gt;")
}

