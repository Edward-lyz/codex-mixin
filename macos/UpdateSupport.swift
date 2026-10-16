import Foundation

enum UpdateLanguage: Equatable {
    case simplifiedChinese
    case english

    static var current: UpdateLanguage {
        let preferred = AppLocalization.preferredLocalization
        return preferred.hasPrefix("zh") ? .simplifiedChinese : .english
    }
}
