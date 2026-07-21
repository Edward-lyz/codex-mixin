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
                let dmgURL = try await downloadUpdate(asset: asset, version: release.version)
                NSWorkspace.shared.open(dmgURL)
            } catch {
                showAlert(title: strings.downloadFailedTitle, message: String(describing: error))
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
