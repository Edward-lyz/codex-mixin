import Cocoa

/// Shared access point for macOS bundle localization.
///
/// Existing call sites still carry Chinese and English fallbacks while they
/// are migrated to Localizable.strings. The bundle's preferred localization
/// is used instead of reading Locale directly, so per-app language overrides
/// in System Settings are respected.
enum AppLocalization {
    static let bundle = Bundle.main

    static var preferredLocalization: String {
        bundle.preferredLocalizations.first?.lowercased() ?? "en"
    }

    static var isChinese: Bool {
        preferredLocalization.hasPrefix("zh")
    }

    static func text(key: String, chinese: String, english: String) -> String {
        let localized = bundle.localizedString(
            forKey: key,
            value: english,
            table: "Localizable"
        )

        // During migration, an untranslated Chinese key should remain Chinese
        // on Chinese systems instead of falling back to the English value.
        if isChinese && localized == english {
            return chinese
        }
        return localized
    }
}
