import Cocoa

struct AgentAsset: Decodable {
    let file: String
    let template: Bool
}

let directory = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
let manifest = try JSONDecoder().decode(
    [String: AgentAsset].self,
    from: Data(contentsOf: directory.appendingPathComponent("manifest.json"))
)
precondition(manifest.count == 50, "Every fixed Agent must have a logo")
for (id, asset) in manifest {
    guard let image = NSImage(contentsOf: directory.appendingPathComponent(asset.file)) else {
        fatalError("Cannot render Agent logo: \(id)")
    }
    precondition(image.size.width > 0 && image.size.height > 0, id)
    image.isTemplate = asset.template
    for appearance in [NSAppearance.Name.aqua, .darkAqua] {
        NSAppearance(named: appearance)?.performAsCurrentDrawingAppearance {
            let rendered = NSImage(size: NSSize(width: 28, height: 28), flipped: false) { rect in
                image.draw(in: rect)
                return true
            }
            precondition(rendered.tiffRepresentation != nil, "Cannot rasterize \(id) in \(appearance)")
        }
    }
}
print("All 50 Agent brand logos render in light and dark appearances")
