# Claude Desktop Chinese catalogs

Source: [javaht/claude-desktop-zh-cn](https://github.com/javaht/claude-desktop-zh-cn),
MIT license, commit `b1330b9aaf2f85e6e4ba05606e70d785686cd588`.

`zh-Hans.json.gz` derives from `resources/frontend-zh-CN.json`;
`zh-Hant.json.gz` derives from `resources/frontend-zh-TW.json`.
Only JSON whitespace was removed before gzip encoding. The catalogs are data;
no upstream installer, injected JavaScript, or model-validation patch is bundled.

The installer merges translated keys into the installed version's English
catalog. New untranslated keys remain in English and are reported in coverage.
