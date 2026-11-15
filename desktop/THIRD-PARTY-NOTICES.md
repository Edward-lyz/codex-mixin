# Desktop dependencies

## TKCode.Arc 1.1.0

MIT License

Copyright (c) 2026 TKCode.Arc contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Avalonia 12.1.5

The MIT License (MIT)

Copyright (c) AvaloniaUI OÜ
All Rights Reserved

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## TKCode.Arc upstream third-party notices

# Third-party notices

TKCode.Arc ports the design and behavior of the free components of Arc and uses the
assets and libraries listed below.

## Arc (uiarc.dev) — free components and blocks

TKCode.Arc reimplements the free (MIT) components of Arc for Avalonia, and `TKCode.Arc.Blocks` ports
Arc's free (MIT) blocks (Page header, Command palette, Notification center, Empty states, Site header, Site footer, CTA section, Newsletter signup, OTP input, Plan comparison and the Email code, Centered and Sign up login blocks). The code,
styles, text and assets of Arc Pro components and blocks are not used, copied or reconstructed. A few Pro
counterparts (the data grid, the Sidebar and Search results blocks, the AI blocks, the Showpieces blocks, the Projects blocks and the
five AI explainers and the five AI components (agent status, tool call, agent plan, agent timeline, context
window) and the six surfaces (sheet stack, wallet stack, control center, holo card, card to article, link unfurl)
and the three cursor components (dot grid, gravity well, ink fluid)
and the three audio components (voice orb, now playing, voice recorder)
and the five loaders (stretch refresh, morph loader, skeleton morph, black hole, charge session)
and the six docks and tabs components (share sheet, action morph, morph nav, tool dock, liquid tab bar, orbit menu) in
`TKCode.Arc.Special`, and the Split and Immersive login blocks, Onboarding illustration and Multi-step form and the seven Pricing blocks (pricing calculator, usage
pricing, cart drawer, checkout summary, live checkout, seat manager, credit top-up) and the Import wizard block in
`TKCode.Arc.Blocks`) are
original TKCode designs that take the behavior and look Arc publishes as a design reference; their code, copy,
sample data and images are written from scratch (the Showpieces' land outlines come from Natural Earth, below).

Other components and blocks in `TKCode.Arc.Special` and `TKCode.Arc.Blocks` share a name with an Arc Pro
item but are independent TKCode designs: they were built from the general idea the name describes, without
looking at the Arc Pro pages, docs, previews, source or assets, so their look and behavior are our own.

```
MIT License

Copyright (c) 2026 Elia Kuratli

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Lucide icons

Icon geometry in `TKCode.Arc.Core` (Glyphs) and `TKCode.Arc.Icons.Lucide` is generated from
lucide-static.

```
ISC License

Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2026 as part of Feather (MIT).
All other copyright (c) for Lucide are held by Lucide Contributors 2026.

Permission to use, copy, modify, and/or distribute this software for any purpose with or
without fee is hereby granted, provided that the above copyright notice and this permission
notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS
SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL
THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY
DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF
CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE
OR PERFORMANCE OF THIS SOFTWARE.
```

## Unicode emoji data — `TKCode.Arc.Special` (EmojiPicker)

`EmojiPicker` embeds a table built by `tools/emoji/generate_emoji.py` from Unicode `emoji-test.txt` 16.0
(https://unicode.org/Public/emoji/16.0/emoji-test.txt) and the English CLDR annotations
(`cldr-json`, `cldr-annotations-full/annotations/en/annotations.json`). Both are Copyright © Unicode, Inc.,
distributed under the Unicode License v3 (https://www.unicode.org/license.txt).

## Natural Earth — land outlines

`TKCode.Arc.Blocks` embeds a 0.5° land mask made from **Natural Earth** 1:110m Physical Vectors, Land
(`ne_110m_land`), for the `RevenueGlobe` block's dotted continents (`tools/geo/generate_land_mask.py` rasterizes the
polygons; the mask is `Blocks/Showpieces/RevenueGlobe/Resources/land-mask.bin`, 32 KB). Natural Earth data is in
the public domain. Made with Natural Earth. Free vector and raster map data at https://www.naturalearthdata.com.

## Fonts

- Inter (SIL Open Font License 1.1), used through the `Avalonia.Fonts.Inter` package.
- Geist and Geist Mono (SIL Open Font License 1.1, Copyright (c) 2023 Vercel, in collaboration with basement.studio), bundled in the optional `TKCode.Arc.Fonts` package; the full license text ships as `licenses/Geist-OFL.txt` (source: `src/TKCode.Arc.Fonts/Assets/OFL.txt`).
- Silkscreen (SIL Open Font License 1.1, Copyright 2001 The Silkscreen Project Authors) and VT323 (SIL Open Font License 1.1, Copyright 2011 The VT323 Project Authors), bundled in `TKCode.Arc.Fonts` for the pixel theme as `fonts:ArcPixel`; the license texts ship as `licenses/Silkscreen-OFL.txt` and `licenses/VT323-OFL.txt` (sources: `src/TKCode.Arc.Fonts/Assets/`).
- Noto Sans SC (SIL Open Font License 1.1, © 2014-2021 Adobe), subset to the characters of the gallery (`tools/fonts/subset-noto-sans-sc.py` in the [ArcGallery](https://github.com/AIDotNet/ArcGallery) repository) as `NotoSansSC-Subset.otf` in the gallery's WebAssembly host, which has no system fonts for Chinese text; the license text ships beside it as `wwwroot/fonts/OFL.txt`.

## StreamMark — Markdown rendering and code highlighting

`TKCode.Arc` references the StreamMark packages (`TKCode.StreamMark.Core`, `TKCode.StreamMark.Avalonia`,
`TKCode.StreamMark.Highlighting.TextMate`) as published NuGet binaries. They render the `Markdown` control and
tokenize the code of `CodeBlock`. StreamMark.Core contains a C# port of remend from vercel/streamdown
(Apache License 2.0, Copyright 2023 Vercel, Inc.); see StreamMark's own third-party notices.

```
MIT License

Copyright (c) 2026 StreamMark contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### TextMateSharp and TextMateSharp.Grammars

TextMate tokenization (dependency of `TKCode.StreamMark.Highlighting.TextMate`).

```
MIT License

Copyright (c) 2021 Daniel Peñalba

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

TextMateSharp.Grammars embeds TextMate grammars, language configurations and color themes, most of them taken
from Visual Studio Code (MIT License, Copyright (c) Microsoft Corporation) and the grammar repositories it
bundles (MIT, TextMate bundle license, Apache-2.0, BSD and GitHub licenses). The `cgmanifest.json` files embedded
in the package record the source and license of each grammar. TKCode.Arc tokenizes with VS Code's Light+ theme
and paints the result in its own palette.

### Onigwrap and Oniguruma

TextMateSharp runs its regular expressions on the native Oniguruma library through Onigwrap (native libraries per
runtime identifier). Onigwrap: MIT License, Copyright (c) 2024 Aikawa Yataro, derived from fluentCODE/onigwrap
(MIT License, Copyright (c) 2015 Fluent Solutions). The Gallery's WebAssembly host (the [ArcGallery](https://github.com/AIDotNet/ArcGallery) repository) links
`gallery/TKCode.Arc.Gallery.Browser/native/onigwrap/<version>/st/libonigwrap.a`, compiled from Onigwrap 1.0.11 and
Oniguruma 6.9.10 by `tools/onigwrap/build-wasm.sh` (the package has no build for the Emscripten of .NET 11). Oniguruma:

```
Copyright (c) 2002-2021  K.Kosako  <kkosako0@gmail.com>
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:
1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.
```

### Markdig

Markdown parsing (dependency of `TKCode.StreamMark.Core`), https://github.com/xoofx/markdig.

```
BSD 2-Clause License

Copyright (c) Alexandre Mutel
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:
1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.
```

Optional StreamMark plugins that an application may add (`TKCode.StreamMark.RaTeX`, `TKCode.StreamMark.Mermaid`)
are not referenced by TKCode.Arc and carry their own notices.

## miniaudio — desktop audio playback

`TKCode.Arc.Audio.Native` compiles miniaudio 0.11.25 (David Reid, https://github.com/mackron/miniaudio) into its native
library. miniaudio is offered as public domain (Unlicense) or MIT No Attribution; TKCode.Arc uses it under MIT-0:

```
Copyright 2026 David Reid

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## PDFium — `TKCode.Arc.Pdf.Native`

`TKCode.Arc.Pdf.Native` ships PDFium's native library and the components it bundles. Only that package carries them, so their notices and license texts travel with it and not with every package: see [`src/TKCode.Arc.Pdf.Native/PDFIUM-NOTICES.md`](src/TKCode.Arc.Pdf.Native/PDFIUM-NOTICES.md) (`PDFIUM-NOTICES.md` and `licenses/` in the package).

## pdf.js — `TKCode.Arc.Pdf.Browser`

`TKCode.Arc.Pdf.Browser` ships pdf.js and the data and decoders it needs. Only that package carries them, so their notices and license texts travel with it and not with every package: see [`src/TKCode.Arc.Pdf.Browser/PDFJS-NOTICES.md`](src/TKCode.Arc.Pdf.Browser/PDFJS-NOTICES.md) (`PDFJS-NOTICES.md` and the files under `staticwebassets/pdfjs/` in the package).

## Dependencies

- Avalonia (MIT).
