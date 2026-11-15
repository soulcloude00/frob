import assert from "node:assert/strict";
import test from "node:test";

import {
  frobRevalidationKey,
  buildFrobCommand,
  installFrob,
  runFrob,
} from "../dist/sandbox/sandbox.js";

test("builds Eve revalidation key from install options", () => {
  assert.equal(
    frobRevalidationKey({ installSpec: "frob@1.2.3" }),
    "frob:bootstrap-4:frob@1.2.3:browser:system-deps",
  );
  assert.equal(
    frobRevalidationKey({ installSpec: "frob@1.2.3", installSystemDependencies: false }),
    "frob:bootstrap-4:frob@1.2.3:browser:no-system-deps",
  );
});

test("builds Eve shell command", () => {
  assert.equal(
    buildFrobCommand(["open", "https://example.com"], { session: "s1" }),
    "frob --session s1 open https://example.com --json",
  );
});

test("installs frob in an Eve sandbox", async () => {
  const commands = [];
  const sandbox = {
    async run({ command }) {
      commands.push(command);
      return { exitCode: 0, stdout: "", stderr: "" };
    },
  };

  await installFrob(sandbox, { installSpec: "frob@1.2.3" });

  assert.equal(commands.length, 3);
  assert.match(commands[0], /^if command -v apt-get/);
  assert.match(commands[0], /libglib2\.0-0t64/);
  assert.match(commands[0], /libasound2t64/);
  assert.match(commands[0], /libnss3-tools/);
  assert.match(commands[0], /sudo ldconfig; elif command -v dnf/);
  assert.match(commands[0], /sudo ldconfig; else echo/);
  assert.match(commands[0], /sudo dnf install -y --skip-broken -- glib2 nss nss-tools/);
  assert.equal(commands[1], "npm install -g frob@1.2.3");
  assert.equal(commands[2], "frob install");
});

test("skips Eve system dependencies when explicitly disabled", async () => {
  const commands = [];
  const sandbox = {
    async run({ command }) {
      commands.push(command);
      return { exitCode: 0, stdout: "", stderr: "" };
    },
  };

  await installFrob(sandbox, {
    installSpec: "frob@1.2.3",
    installSystemDependencies: false,
  });

  assert.deepEqual(commands, ["npm install -g frob@1.2.3", "frob install"]);
});

test("runs frob through ctx.getSandbox", async () => {
  const commands = [];
  const ctx = {
    async getSandbox() {
      return {
        async run({ command }) {
          commands.push(command);
          return { exitCode: 0, stdout: '{"ok":true}', stderr: "" };
        },
      };
    },
  };

  const result = await runFrob(ctx, ["open", "https://example.com"]);

  assert.deepEqual(result.json, { ok: true });
  assert.equal(commands[0], "frob --session eve-default open https://example.com --json");
});

test("accepts Eve promise-like sandbox methods", async () => {
  const thenable = (value) => ({
    then(resolve) {
      resolve(value);
    },
  });
  const ctx = {
    getSandbox() {
      return thenable({
        run() {
          return thenable({ exitCode: 0, stdout: '{"ok":true}', stderr: "" });
        },
      });
    },
  };

  const result = await runFrob(ctx, ["snapshot"]);

  assert.deepEqual(result.json, { ok: true });
});

test("throws when Eve sandbox command fails", async () => {
  const ctx = {
    async getSandbox() {
      return {
        async run() {
          return { exitCode: 2, stdout: "", stderr: "no chrome" };
        },
      };
    },
  };

  await assert.rejects(() => runFrob(ctx, ["snapshot"]), /no chrome/);
});
