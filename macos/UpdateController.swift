import Cocoa

extension AppDelegate {
    @MainActor
    @objc func checkForUpdatesFromMenu() {
        Task { @MainActor in
            await checkForUpdates(interactive: true)
        }
    }

    @MainActor
    func checkForUpdates(interactive: Bool) async {
        let strings = UpdateStrings.current
        let release: GitHubRelease
        do {
            release = try await fetchLatestRelease()
        } catch {
            if interactive {
                showAlert(title: strings.checkFailedTitle, message: String(describing: error))
            }
            return
        }
        let currentVersion = appVersion()
        guard compareVersions(release.version, currentVersion) == .orderedDescending else {
            if interactive {
                showAlert(
                    title: strings.upToDateTitle,
                    message: strings.upToDateMessage(current: currentVersion, latest: release.version)
                )
            }
            return
        }
        let asset = release.assets.first {
            $0.name == expectedDMGAssetName(version: release.version)
        }
        let action = presentUpdatePrompt(
            release: release,
            currentVersion: currentVersion,
            assetAvailable: asset != nil,
            strings: strings
        )
        switch action {
        case .download:
            guard let asset else {
                NSWorkspace.shared.open(release.htmlURL)
                return
            }
            do {
                try await runOperationProgress(
                    title: "正在下载更新",
                    phases: [
                        "下载更新包",
                        "停止本地网关",
                        "替换应用",
                        "重新启动",
                    ],
                    detail: asset.name,
                    successTitle: "✓ 下载完成",
                    failureTitle: "✗ 下载失败",
                    showFailureAlert: true,
                    failureAlertTitle: strings.downloadFailedTitle
                ) { progress in
                    progress.advance(to: 0)
                    let dmgURL = try await downloadUpdate(
                        asset: asset,
                        version: release.version
                    )
                    progress.advance(to: 1)
                    _ = try await runGateway(["stop"])
                    progress.advance(to: 2)
                    try await installDownloadedUpdate(dmgURL)
                    progress.advance(to: 3)
                }
            } catch {
                // Failure already shown by the progress window + alert.
            }
        case .releasePage:
            NSWorkspace.shared.open(release.htmlURL)
        case .later:
            break
        }
    }

    @MainActor
    func presentUpdatePrompt(
        release: GitHubRelease,
        currentVersion: String,
        assetAvailable: Bool,
        strings: UpdateStrings
    ) -> UpdatePromptAction {
        let alert = NSAlert()
        alert.messageText = strings.updateAvailableTitle(version: release.version)
        alert.informativeText = strings.versionSummary(
            current: currentVersion,
            latest: release.version,
            assetAvailable: assetAvailable
        )
        alert.alertStyle = .informational
        if assetAvailable {
            alert.addButton(withTitle: strings.downloadButton)
            alert.addButton(withTitle: strings.releasePageButton)
            alert.addButton(withTitle: strings.laterButton)
        } else {
            alert.addButton(withTitle: strings.releasePageButton)
            alert.addButton(withTitle: strings.laterButton)
        }
        alert.accessoryView = releaseNotesView(
            title: strings.whatsNewTitle,
            notes: release.localizedNotes(
                language: strings.language,
                fallback: strings.noReleaseNotes
            )
        )
        NSApp.activate(ignoringOtherApps: true)
        let response = alert.runModal()
        if assetAvailable {
            switch response {
            case .alertFirstButtonReturn: return .download
            case .alertSecondButtonReturn: return .releasePage
            default: return .later
            }
        }
        return response == .alertFirstButtonReturn ? .releasePage : .later
    }

    func fetchLatestRelease() async throws -> GitHubRelease {
        var request = URLRequest(url: URL(string: "https://api.github.com/repos/Edward-lyz/codex-mixin/releases/latest")!)
        request.setValue("Codex Mixin", forHTTPHeaderField: "User-Agent")
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let httpResponse = response as? HTTPURLResponse, httpResponse.statusCode == 200 else {
            throw GatewayError.command("GitHub release API returned a non-200 response")
        }
        return try JSONDecoder().decode(GitHubRelease.self, from: data)
    }

    func downloadUpdate(asset: GitHubRelease.Asset, version: String) async throws -> URL {
        let downloads = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first
            ?? FileManager.default.homeDirectoryForCurrentUser
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

    @MainActor
    func installDownloadedUpdate(_ dmgURL: URL) async throws {
        let mountPoint = FileManager.default.temporaryDirectory
            .appendingPathComponent("codex-mixin-update-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: mountPoint, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: mountPoint)
            try? FileManager.default.removeItem(at: dmgURL)
        }

        _ = try await runProcess(
            "/usr/bin/hdiutil",
            ["attach", dmgURL.path, "-nobrowse", "-readonly", "-mountpoint", mountPoint.path]
        )
        defer {
            Task {
                _ = try? await runProcess("/usr/bin/hdiutil", ["detach", mountPoint.path, "-force"])
            }
        }

        guard let appURL = FileManager.default.enumerator(
            at: mountPoint,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        )?.compactMap({ $0 as? URL }).first(where: {
            $0.pathExtension == "app" && $0.lastPathComponent == "Codex Mixin.app"
        }) else {
            throw GatewayError.command("update image does not contain Codex Mixin.app")
        }

        let destination = Bundle.main.bundleURL
        guard destination.pathExtension == "app" else {
            throw GatewayError.command("current application bundle path is invalid")
        }
        let stagingURL = destination.deletingLastPathComponent()
            .appendingPathComponent(".Codex Mixin.update-\(UUID().uuidString).app")
        defer { try? FileManager.default.removeItem(at: stagingURL) }
        _ = try await runProcess("/usr/bin/ditto", ["--rsrc", appURL.path, stagingURL.path])
        _ = try FileManager.default.replaceItemAt(destination, withItemAt: stagingURL)
        guard FileManager.default.fileExists(atPath: destination.path) else {
            throw GatewayError.command("updated application bundle is missing after replacement")
        }
        NSWorkspace.shared.open(destination)
        NSApp.terminate(nil)
    }

    func appVersion() -> String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.0.0"
    }

    func expectedDMGAssetName(version: String) -> String {
        "codex-mixin-\(version)-\(macTargetTriple()).dmg"
    }

    func macTargetTriple() -> String {
        var systemInfo = utsname()
        uname(&systemInfo)
        let machine = withUnsafePointer(to: &systemInfo.machine) {
            $0.withMemoryRebound(to: CChar.self, capacity: 1) {
                String(cString: $0)
            }
        }
        return machine == "arm64" ? "aarch64-apple-darwin" : "x86_64-apple-darwin"
    }
}
