import Cocoa

@main
struct InstallProgressWindowTests {
    static func main() {
        _ = NSApplication.shared

        precondition(
            localizedProgressLabel("Fetching available models") == "获取可用模型列表"
        )
        precondition(
            localizedProgressLabel("检查本地配置与网关状态") == "检查本地配置与网关状态"
        )
        precondition(
            localizedProgressLabel("unknown progress line") == "unknown progress line"
        )

        let replay = try! decodeDUCXReplayReport(
            #"{"queued_from_local_sessions":3,"delivered":[{"provider_id":"baidu-oneapi","session_id":"session-ok","event":"post-tool-use"}],"retained":[{"provider_id":"baidu-oneapi","session_id":"session-failed","event":"post-tool-use","error":"upload/code/accept returned 500"}]}"#
        )
        let replayText = formatDUCXReplayReport(replay)
        precondition(replayText.contains("上传成功：1"))
        precondition(replayText.contains("[OK] 代码采纳 · baidu-oneapi · session-ok"))
        precondition(replayText.contains("上传失败并保留重试：1"))
        precondition(replayText.contains("[ERROR] 代码采纳 · baidu-oneapi · session-failed"))
        precondition(replayText.contains("upload/code/accept returned 500"))

        let releaseNotes = releaseNotesView(title: "更新内容", notes: "修复更新窗口布局")
        precondition(
            releaseNotes.frame.size == NSSize(width: 560, height: 300),
            "release notes accessory must have an explicit AppKit frame"
        )
        let updateAlert = NSAlert()
        updateAlert.messageText = "发现新版本"
        updateAlert.informativeText = "当前版本 0.5.0，最新版本 0.5.1"
        updateAlert.addButton(withTitle: "稍后")
        updateAlert.accessoryView = releaseNotes
        updateAlert.layout()
        precondition(
            updateAlert.window.frame.width >= 560 && updateAlert.window.frame.height >= 300,
            "update alert must not collapse around its release notes"
        )

        let controller = InstallProgressWindowController(
            title: "测试进度",
            detail: "detail",
            successTitle: "✓ done",
            failureTitle: "✗ fail"
        )
        controller.setPhases([
            "第一步",
            "第二步",
            "第三步",
        ])
        controller.present()
        controller.advance(to: 0)
        controller.advanceStreamedPhase("Fetching available models")
        controller.advanceStreamedPhase("Loading model metadata")
        controller.advanceStreamedPhase("Writing Codex config and model catalog")
        // Completed operations remain visible until the user closes them.
        var retained: InstallProgressWindowController? = controller
        retained?.finish()
        RunLoop.current.run(until: Date().addingTimeInterval(1.0))
        precondition(retained?.window?.isVisible == true, "success progress window must remain visible")
        precondition(retained?.window?.styleMask.contains(.closable) == true)
        precondition(!(retained?.window is NSPanel), "progress must use a persistent NSWindow")
        retained?.close()
        retained = nil

        let failing = InstallProgressWindowController(
            title: "失败路径",
            successTitle: "✓ done",
            failureTitle: "✗ fail"
        )
        failing.setPhases(["准备", "执行"])
        failing.present()
        failing.fail(message: "boom")
        precondition(failing.window?.styleMask.contains(.closable) == true)

        print("Install progress window tests passed")
    }
}
