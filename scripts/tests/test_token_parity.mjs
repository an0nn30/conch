// Run: node scripts/tests/test_token_parity.mjs
//
// Guards against silent dark-value fallthrough in the light theme:
// tokens-dark.css is the unconditional :root block and tokens-light.css only
// overrides it, so any raw token the semantic alias layer (base.css) consumes
// WITHOUT an explicit var() fallback must be defined in BOTH generated token
// sets — otherwise the alias silently resolves to the dark value under
// data-tl-appearance="light". References that carry an explicit fallback are
// deliberate (documented in base.css) and exempt from the both-sets rule,
// but must still resolve somewhere (defined in at least one set) so the
// fallback is a real safety net rather than the only value.
import assert from 'node:assert';
import { readFileSync } from 'node:fs';

const DS = 'crates/termlab_tauri/frontend/styles/design-system';
const baseCss = readFileSync(`${DS}/base.css`, 'utf8');
const darkCss = readFileSync(`${DS}/tokens-dark.css`, 'utf8');
const lightCss = readFileSync(`${DS}/tokens-light.css`, 'utf8');

const definedIn = (css) => new Set(
  [...css.matchAll(/^\s*(--tl-[\w-]+)\s*:/gm)].map((m) => m[1])
);
const dark = definedIn(darkCss);
const light = definedIn(lightCss);
const aliases = definedIn(baseCss);

assert.ok(dark.size > 0, 'tokens-dark.css defines no --tl-* tokens');
assert.ok(light.size > 0, 'tokens-light.css defines no --tl-* tokens');

// var(--tl-x) → no fallback; var(--tl-x, ...) → has fallback. Aliases defined
// by base.css itself (including inside fallback expressions) are not raw
// tokens and are skipped.
const refs = [...baseCss.matchAll(/var\(\s*(--tl-[\w-]+)\s*([,)])/g)]
  .map((m) => ({ token: m[1], hasFallback: m[2] === ',' }))
  .filter((r) => !aliases.has(r.token));

assert.ok(refs.length > 0, 'base.css consumes no raw --tl-* tokens');

const missing = new Set();
for (const { token, hasFallback } of refs) {
  if (hasFallback) {
    assert.ok(
      dark.has(token) || light.has(token),
      `${token} has a fallback but is defined in neither token set — phantom reference`
    );
    continue;
  }
  for (const [setName, set] of [['tokens-dark.css', dark], ['tokens-light.css', light]]) {
    if (!set.has(token)) missing.add(`${token} missing from ${setName}`);
  }
}

assert.deepEqual(
  [...missing],
  [],
  `raw tokens consumed by base.css aliases without a fallback must exist in both token sets:\n  ${[...missing].join('\n  ')}`
);
console.log('ok');
