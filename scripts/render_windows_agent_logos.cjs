// Run with sharp@0.35.5 available through NODE_PATH. WinUI's SVG subset
// omits the filters used by these logos; retain the original SVGs for macOS.
const path = require("node:path");
const sharp = require("sharp");

async function main() {
  if (sharp.versions.sharp !== "0.35.5") {
    throw new Error("Logo regeneration requires sharp@0.35.5");
  }
  const assets = path.resolve(__dirname, "../macos/assets/agents");
  for (const id of ["agy", "workbuddy"]) {
    await sharp(path.join(assets, `${id}.svg`), { density: 384 })
      .resize(128, 128, { fit: "contain", background: "#00000000" })
      .png()
      .toFile(path.join(assets, `${id}.png`));
  }
}

main().catch((error) => { console.error(error.message); process.exitCode = 1; });
