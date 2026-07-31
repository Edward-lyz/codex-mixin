import Cocoa

func menuItemImage(_ systemSymbolName: String) -> NSImage? {
    nil
}

@main
struct DuccManagedInstallTests {
    static func main() async throws {
        let fileManager = FileManager.default
        let temporaryRoot = fileManager.temporaryDirectory.appendingPathComponent(
            "codex-mixin-ducc-install-test-\(UUID().uuidString)",
            isDirectory: true
        )
        try fileManager.createDirectory(
            at: temporaryRoot,
            withIntermediateDirectories: false
        )
        defer { try? fileManager.removeItem(at: temporaryRoot) }

        let environment = ProcessInfo.processInfo.environment
        let archive: URL
        let format: DuccArchiveFormat
        let version: String
        if let archivePath = environment["DUCC_TEST_ARCHIVE"] {
            archive = URL(fileURLWithPath: archivePath)
            format = environment["DUCC_TEST_FORMAT"] == "bzip2" ? .bzip2 : .zstd
            version = environment["DUCC_TEST_VERSION"] ?? "2.1.218.3"
        } else {
            version = "1.2.3"
            format = .bzip2
            archive = temporaryRoot.appendingPathComponent("fixture.tar.bz2")
            try createFixtureArchive(
                at: archive,
                version: version,
                under: temporaryRoot
            )
        }

        let managedRoot = environment["DUCC_TEST_MANAGED_ROOT"].map {
            URL(fileURLWithPath: $0, isDirectory: true)
        } ?? temporaryRoot.appendingPathComponent(
            "managed-home/.baidu-cc",
            isDirectory: true
        )
        if environment["DUCC_TEST_MANAGED_ROOT"] == nil {
            try fileManager.createDirectory(
                at: managedRoot,
                withIntermediateDirectories: true
            )
            let loginState = managedRoot.appendingPathComponent("user.json")
            let metadata = managedRoot.appendingPathComponent("meta.json")
            try "login-state\n".write(
                to: loginState,
                atomically: true,
                encoding: .utf8
            )
            try "metadata\n".write(
                to: metadata,
                atomically: true,
                encoding: .utf8
            )
            try fileManager.createDirectory(
                at: managedRoot.appendingPathComponent(
                    "baidu-cc-darwin-arm64-0.9.0",
                    isDirectory: true
                ),
                withIntermediateDirectories: true
            )
            try fileManager.createDirectory(
                at: managedRoot.appendingPathComponent(
                    ".install-interrupted",
                    isDirectory: true
                ),
                withIntermediateDirectories: true
            )
            let incompleteVersion = managedRoot.appendingPathComponent(
                "baidu-cc-darwin-arm64-\(version)",
                isDirectory: true
            )
            try fileManager.createDirectory(
                at: incompleteVersion,
                withIntermediateDirectories: true
            )
            try "interrupted install\n".write(
                to: incompleteVersion.appendingPathComponent("incomplete"),
                atomically: true,
                encoding: .utf8
            )
        }
        let release = DuccRelease(
            version: version,
            zstdArchiveURL: URL(string: "http://example.invalid/fixture.tar.zst")!,
            bzip2ArchiveURL: URL(string: "http://example.invalid/fixture.tar.bz2")!
        )
        let executable = try await installDuccArchive(
            archive,
            release: release,
            format: format,
            root: managedRoot,
            architecture: "arm64"
        )
        precondition(
            fileManager.isExecutableFile(atPath: executable.path),
            "Managed DUCC executable must be executable"
        )
        precondition(
            (try? fileManager.destinationOfSymbolicLink(
                atPath: managedRoot.appendingPathComponent("baidu-cc").path
            )) == "baidu-cc-darwin-arm64-\(version)",
            "Managed DUCC official entry must point at the installed version"
        )
        precondition(
            !fileManager.fileExists(
                atPath: managedRoot
                    .appendingPathComponent("baidu-cc/settings.json")
                    .path
            ),
            "Bundled DUCC settings must not enter the managed runtime"
        )
        if environment["DUCC_TEST_MANAGED_ROOT"] == nil {
            let loginStateContents = try String(
                contentsOf: managedRoot.appendingPathComponent("user.json"),
                encoding: .utf8
            )
            let metadataContents = try String(
                contentsOf: managedRoot.appendingPathComponent("meta.json"),
                encoding: .utf8
            )
            precondition(
                loginStateContents == "login-state\n",
                "DUCC cleanup must preserve login state"
            )
            precondition(
                metadataContents == "metadata\n",
                "DUCC cleanup must preserve metadata"
            )
            precondition(
                !fileManager.fileExists(
                    atPath: managedRoot
                        .appendingPathComponent("baidu-cc-darwin-arm64-0.9.0")
                        .path
                )
                    && !fileManager.fileExists(
                        atPath: managedRoot.appendingPathComponent(
                            ".install-interrupted"
                        ).path
                    ),
                "DUCC cleanup must remove old versions and interrupted staging"
            )
        }
        print("Managed DUCC archive installation: passed")
    }

    private static func createFixtureArchive(
        at archive: URL,
        version: String,
        under temporaryRoot: URL
    ) throws {
        let fileManager = FileManager.default
        let payload = temporaryRoot.appendingPathComponent("payload", isDirectory: true)
        let bin = payload.appendingPathComponent("bin", isDirectory: true)
        try fileManager.createDirectory(
            at: bin,
            withIntermediateDirectories: true
        )
        let claude = bin.appendingPathComponent("claude")
        try "#!/bin/sh\nexit 0\n".write(
            to: claude,
            atomically: true,
            encoding: .utf8
        )
        try fileManager.setAttributes(
            [.posixPermissions: 0o755],
            ofItemAtPath: claude.path
        )
        try "\(version)\n".write(
            to: payload.appendingPathComponent("version"),
            atomically: true,
            encoding: .utf8
        )
        try "{}\n".write(
            to: payload.appendingPathComponent("settings.json"),
            atomically: true,
            encoding: .utf8
        )

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/tar")
        process.arguments = [
            "-cjf",
            archive.path,
            "-C",
            payload.path,
            ".",
        ]
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()
        precondition(
            process.terminationStatus == 0,
            "Unable to create DUCC archive fixture"
        )
    }
}
