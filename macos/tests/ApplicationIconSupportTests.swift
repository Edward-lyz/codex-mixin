import Cocoa

@main
struct ApplicationIconSupportTests {
    static func main() {
        guard let lightAppearance = NSAppearance(named: .aqua),
              let darkAppearance = NSAppearance(named: .darkAqua)
        else {
            fatalError("System Aqua appearances are unavailable")
        }

        precondition(applicationIconResourceName(for: lightAppearance) == "CodexMixin")
        precondition(applicationIconResourceName(for: darkAppearance) == "CodexMixinDark")
    }
}
