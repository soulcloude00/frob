import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  FROB_SANDBOX_VERSION,
  DEFAULT_FROB_INSTALL_SPEC,
  buildFrobArgv,
  buildShellCommand,
  defaultSessionName,
  quoteShellArg,
  resolveFrobInstallSpec,
} from "../dist/index.js";

test("defaults install spec to the package version", () => {
  const packageJson = JSON.parse(
    readFileSync(new URL("../package.json", import.meta.url), "utf-8"),
  );
  const expectedInstallSpec = `frob@${packageJson.version}`;

  assert.equal(FROB_SANDBOX_VERSION, packageJson.version);
  assert.equal(DEFAULT_FROB_INSTALL_SPEC, expectedInstallSpec);
  assert.equal(resolveFrobInstallSpec(), expectedInstallSpec);
  assert.equal(resolveFrobInstallSpec({ installSpec: "latest" }), "latest");
});

test("builds argv with session and json by default", () => {
  assert.deepEqual(buildFrobArgv(["open", "https://example.com"], { session: "s1" }), [
    "--session",
    "s1",
    "open",
    "https://example.com",
    "--json",
  ]);

  assert.deepEqual(buildFrobArgv(["snapshot", "--json"], { session: "s1" }), [
    "--session",
    "s1",
    "snapshot",
    "--json",
  ]);
});

test("quotes shell args and builds commands", () => {
  assert.equal(quoteShellArg("simple"), "simple");
  assert.equal(quoteShellArg("hello world"), "'hello world'");
  assert.equal(quoteShellArg("can't"), "'can'\\''t'");

  assert.equal(
    buildShellCommand(["open", "https://example.com/a b"], {
      env: { FROB_HOME: "/tmp/frob" },
      session: "s1",
    }),
    "FROB_HOME=/tmp/frob frob --session s1 open 'https://example.com/a b' --json",
  );
});

test("sanitizes default session names", () => {
  assert.equal(defaultSessionName("eve", "sandbox/id 1"), "eve-sandbox-id-1");
});

test("keeps generated default session names short", () => {
  const session = defaultSessionName(
    "eve",
    "eve-sbx-ses-vercel-1d940340bdba4563-wrun_01KVKDK1Z3GC3XEC86DGWRWRMH-__root__",
  );

  assert.equal(session.length <= 48, true);
  assert.match(session, /^eve-eve-sbx-ses-vercel-.+-[a-f0-9]{8}$/);
});
