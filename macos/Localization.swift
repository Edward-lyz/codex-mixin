import Cocoa

/// Shared access point for macOS bundle localization.
///
/// The bundle's preferred localization is used instead of reading Locale
/// directly, so per-app language overrides in System Settings are respected.
enum AppLocalization {
    static let bundle = Bundle.main
    private static let testStrings: [String: String]? = {
        guard let path = ProcessInfo.processInfo.environment["CODEX_MIXIN_LOCALIZATION_DIR"] else {
            return nil
        }
        return NSDictionary(contentsOfFile: "\(path)/en.lproj/Localizable.strings") as? [String: String]
    }()

    static var preferredLocalization: String {
        bundle.preferredLocalizations.first?.lowercased() ?? "en"
    }

    static func string(_ key: String, _ arguments: CVarArg...) -> String {
        let format = testStrings?[key]
            ?? bundle.localizedString(forKey: key, value: nil, table: "Localizable")
        guard !arguments.isEmpty else { return format }
        return String(format: format, locale: Locale.current, arguments: arguments)
    }
}
