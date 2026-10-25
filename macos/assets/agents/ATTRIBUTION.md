# Agent Logos

The agent logos are copied from the icon set in magpie
(https://github.com/yetone/magpie), licensed under the MIT License,
Copyright (c) 2026 yetone. Magpie takes them from LobeHub Icons
(https://github.com/lobehub/lobe-icons), licensed under the MIT License.

`dsh.svg` is the DeepSeek logo, since DeepSeek Harness has no logo of its
own. The marks remain trademarks of their owners.

The 50 fixed Agent IDs use `manifest.json` as their asset and template-color
mapping. Forks reuse their parent's brand mark (Cursor local, VS Code
Insiders, VSCodium, Qoder CN, and Copilot for JetBrains).

`agy.png` and `workbuddy.png` are 128px raster derivatives of the adjacent
original SVGs for Windows, whose SVG renderer omits their filters. Regenerate
with `scripts/render_windows_agent_logos.cjs` and sharp 0.35.5 available through
`NODE_PATH`. They retain the same source and license; macOS keeps the SVGs.

`ante.svg` comes from the favicon declared by https://antigma.ai
(https://antigma.ai/img/favicon.svg). `pencil.svg` comes from the favicon
declared by https://pen.dev (https://pen.dev/favicon.svg). These identify
the applications; the marks remain trademarks of their respective owners.
