#!/usr/bin/env node

const { chromium } = require("playwright");

const cdpUrl = process.argv[2] ?? "http://127.0.0.1:9333";
const expectedModelPattern =
  /Claude Sonnet|DeepSeek|Fable|GLM|gpt-5\.6|Opus/;

async function main() {
  const browser = await chromium.connectOverCDP(cdpUrl);
  try {
    const page = browser
      .contexts()
      .flatMap((context) => context.pages())
      .find((candidate) => candidate.url() === "app://-/index.html");

    if (!page) {
      throw new Error(`Codex main window not found via ${cdpUrl}`);
    }

    for (let index = 0; index < 3; index += 1) {
      await page.keyboard.press("Escape");
    }
    const composerMenuButton = page
      .locator('button[aria-haspopup="menu"]')
      .filter({
        hasText:
          /(?:自定义|Custom|Claude Sonnet|DeepSeek|Fable|GLM|gpt-5\.6|Opus)[\s\S]*(?:低|中|高|超高|Low|Medium|High)/,
      })
      .last();

    await composerMenuButton.click();
    const modelMenuItem = page
      .getByRole("menuitem")
      .filter({ hasText: /^(?:模型|Model)/ })
      .first();
    await modelMenuItem.hover();
    await page.waitForTimeout(300);

    const menuText = await page
      .getByRole("menu")
      .evaluateAll((menus) => menus.map((menu) => menu.innerText));
    const modelHits = menuText.filter((text) =>
      expectedModelPattern.test(text),
    );

    if (modelHits.length === 0) {
      console.error(
        `FAIL: Codex Desktop model submenu contains no custom models.\n${JSON.stringify(menuText, null, 2)}`,
      );
      process.exitCode = 1;
      return;
    }

    const initialModelText = await composerMenuButton.innerText();
    const targetModel = /DeepSeek-V4-Pro/.test(initialModelText)
      ? "Fable 5"
      : "DeepSeek-V4-Pro";
    const selectableModel = page
      .getByRole("menuitem")
      .filter({ hasText: new RegExp(`^${targetModel.replace(" ", "\\s")}`) })
      .first();
    await selectableModel.evaluate((element) => element.click());
    await page.waitForTimeout(200);
    const selectedModelText = await composerMenuButton.innerText();
    if (!selectedModelText.includes(targetModel)) {
      console.error(
        `FAIL: custom model menu was visible but selecting ${targetModel} did not update the composer.\n${selectedModelText}`,
      );
      process.exitCode = 1;
      return;
    }

    console.log(
      `PASS: Codex Desktop model submenu exposes custom models and selection works.\n${JSON.stringify(modelHits, null, 2)}`,
    );
  } finally {
    await browser.close();
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
