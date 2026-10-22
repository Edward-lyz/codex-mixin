// Run: bun tests/fixtures/context_nudge/generate.ts /path/to/billion-context
// Redirect stdout to expected.json. Requires the pinned upstream checkout.
import { resolve, join } from "node:path";
import { writeFile, unlink } from "node:fs/promises";

const revision = "94f75ccea1b2e509a29ca46e44768dcf9364940f";
const root = resolve(process.argv[2] ?? "");
const git = Bun.spawnSync(["git", "-C", root, "rev-parse", "HEAD"]);
if (git.exitCode !== 0 || git.stdout.toString().trim() !== revision) {
    throw new Error(`Expected billion-context revision ${revision}`);
}
// Expose the original node without rewriting any policy or state logic.
const source = join(root, "kernel/src/compress.ts");
const entry = join(root, `kernel/src/nudge-reference-${process.pid}.ts`);
await writeFile(entry, await Bun.file(source).text() + "\nexport { nudgeNode };\n");
try {
    const { nudgeNode } = await import(entry);
    const { defaultConfig } = await import(join(root, "kernel/src/config.ts"));
    const { createInitialState } = await import(join(root, "kernel/src/state.ts"));
    const cases: unknown[] = [];
    let seed = 20261022;
    const random = (bound: number) => {
        seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
        return (seed >>> 8) % bound;
    };
    const tokens = [0, 22499, 22500, 65000, 224999, 225000, 374999, 375000, 474999, 475000];
    const masses = [0, 4999, 5000, 49999, 50000, 74999, 75000, 100000];
    // Independent decisions plus multi-turn replay through the real stamping
    // node cover resets, ignored nudges, lower samples and per-tier cadence.
    for (let sequence = 0; sequence < 32; sequence++) {
        let state = createInitialState();
        for (let turn = 0; turn < 8; turn++) {
            if (turn === 4) state = createInitialState();
            const before = {
                baseline: state.nudge.lastPerMessageNudgeTokens,
                shown: state.nudge.lastNudgeShownTokens,
                by_tier: [1, 2, 3].map(t => state.nudge.lastShownByTier[t] ?? 0),
            };
            const total = tokens[random(tokens.length)] + random(2);
            const window = [0, 500000, 1000000][random(3)];
            const pending = [0, 1, 2].map(() => masses[random(masses.length)]);
            const counts = [0, [2, 999, 1000][random(3)], [2, 1999, 2000][random(3)]];
            state.blocks = [1, 2].flatMap(tier => Array.from({length: counts[tier]}, (_, index) => ({
                blockId: `b${tier}-${index}`, tier, active: true,
                summary: String(index === 0 ? pending[tier] : 0),
                effectiveMessageIds: [],
            })));
            const recommendation = {
                recommendedRanges: [{ startRef: "m1", endRef: "m2", tokens: pending[0], chars: 5000 }],
                contextRanges: { protected: [] },
            };
            const result = nudgeNode.run({state, messages: [], effects: {recommendation}}, {
                tokenCount: total, config: defaultConfig(window), countTokens: Number,
            });
            state = result.state;
            cases.push({tokens: total, window, pending, counts, before,
                tier: result.effects.nudge.tier,
                after: {baseline: state.nudge.lastPerMessageNudgeTokens,
                    shown: state.nudge.lastNudgeShownTokens,
                    by_tier: [1, 2, 3].map(t => state.nudge.lastShownByTier[t] ?? 0)},
            });
        }
    }
    process.stdout.write(`{\n  "revision": "${revision}",\n  "cases": [\n${cases.map(c => "    " + JSON.stringify(c)).join(",\n")}\n  ]\n}\n`);
} finally {
    await unlink(entry);
}
