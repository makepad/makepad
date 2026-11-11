import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// Only this one function, so the test doesn't need the rest of web.js to load.
const source = readFileSync(new URL("./web.js", import.meta.url), "utf8");
const body = source.match(/^export function makepad_primary_modifier_is_logo\([\s\S]*?\n}\n/m)[0];
const primary_modifier_is_logo = new Function(
  `${body.replace(/^export /, "")}\nreturn makepad_primary_modifier_is_logo;`,
)();

test("a browser on a Mac, iPhone or iPad takes Command and every other browser takes Control", () => {
  const apple = [
    { userAgentData: { platform: "macOS" }, platform: "MacIntel" },
    { platform: "MacIntel" },
    { platform: "iPhone" },
    { platform: "iPad" },
    { userAgentData: { platform: "" }, platform: "", userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)" },
  ];
  const other = [
    { userAgentData: { platform: "Windows" }, platform: "Win32" },
    { platform: "Win32" },
    { platform: "Linux x86_64" },
    { userAgentData: { platform: "Android" }, platform: "Linux armv8l" },
    { userAgentData: { platform: "Chrome OS" }, platform: "Linux x86_64" },
    { platform: "", userAgent: "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 Safari/537.36" },
    {},
  ];
  for (const nav of apple) {
    assert.equal(primary_modifier_is_logo(nav), true, JSON.stringify(nav));
  }
  for (const nav of other) {
    assert.equal(primary_modifier_is_logo(nav), false, JSON.stringify(nav));
  }
});
