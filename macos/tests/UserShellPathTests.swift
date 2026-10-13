import Foundation

private func expect(_ condition: Bool, _ message: String) {
    guard condition else {
        FileHandle.standardError.write(Data("FAIL: \(message)\n".utf8))
        exit(1)
    }
}

@main
struct UserShellPathTests {
    static func main() {
        let existing: Set<String> = ["/Users/u/.local/bin", "/opt/homebrew/bin"]
        let merged = resolveUserShellPATH(
            current: "/usr/bin:/bin",
            loginShellPATH: "/opt/homebrew/bin:/usr/bin",
            home: "/Users/u",
            folderExists: existing.contains
        )
        expect(
            merged == "/opt/homebrew/bin:/usr/bin:/bin:/Users/u/.local/bin",
            "login shell first, duplicates removed, known folders appended: \(merged)"
        )

        let withoutShell = resolveUserShellPATH(
            current: "/usr/bin:/bin",
            loginShellPATH: nil,
            home: "/Users/u",
            folderExists: existing.contains
        )
        expect(
            withoutShell == "/usr/bin:/bin:/Users/u/.local/bin:/opt/homebrew/bin",
            "known folders still apply without a login shell: \(withoutShell)"
        )

        // The real login shell must answer with a PATH that has /usr/bin.
        expect(
            userShellPATH.split(separator: ":").contains("/usr/bin"),
            "resolved PATH keeps system folders: \(userShellPATH)"
        )
        print("UserShellPathTests passed")
    }
}
