import assert from "node:assert/strict";
import test from "node:test";

import {
  CHROMIUM_SYSTEM_DEPS,
  createFrobSandbox,
  createFrobSnapshot,
  getSandboxCredentials,
  installFrobInVercelSandbox,
  runFrobCommand,
  withFrobSandbox,
} from "../dist/vercel.js";

test("default Vercel dependencies include certutil", () => {
  assert.ok(CHROMIUM_SYSTEM_DEPS.includes("nss-tools"));
});

function commandResult(stdout = "{}", stderr = "", exitCode = 0) {
  return {
    exitCode,
    async stderr() {
      return stderr;
    },
    async stdout() {
      return stdout;
    },
  };
}

test("imports vercel entry without loading a real @vercel/sandbox module", async () => {
  assert.equal(typeof getSandboxCredentials, "function");
});

test("reads explicit Vercel credentials", () => {
  assert.deepEqual(
    getSandboxCredentials({
      VERCEL_PROJECT_ID: "project",
      VERCEL_TEAM_ID: "team",
      VERCEL_TOKEN: "token",
    }),
    { projectId: "project", teamId: "team", token: "token" },
  );
  assert.deepEqual(getSandboxCredentials({}), {});
});

test("runs frob command in a Vercel sandbox", async () => {
  const calls = [];
  const sandbox = {
    async runCommand(command, args) {
      calls.push([command, args]);
      return commandResult('{"ok":true}');
    },
  };

  const result = await runFrobCommand(sandbox, ["open", "https://example.com"], {
    session: "s1",
  });

  assert.deepEqual(result.json, { ok: true });
  assert.deepEqual(calls, [
    ["frob", ["--session", "s1", "open", "https://example.com", "--json"]],
  ]);
});

test("creates and bootstraps a fresh Vercel sandbox", async () => {
  const calls = [];
  const sandbox = {
    async runCommand(command, args) {
      calls.push([command, args]);
      return commandResult();
    },
    async snapshot() {
      return { snapshotId: "snap" };
    },
    async stop() {},
  };
  const Sandbox = {
    async create(options) {
      calls.push(["create", options]);
      return sandbox;
    },
  };

  await createFrobSandbox({
    Sandbox,
    env: {},
    install: { installSpec: "frob@1.2.3", systemDependencies: ["nss"] },
  });

  assert.equal(calls[0][0], "create");
  assert.deepEqual(calls[1], [
    "sh",
    [
      "-c",
      "sudo dnf clean all 2>&1 && sudo dnf install -y --skip-broken -- nss 2>&1 && sudo ldconfig 2>&1",
    ],
  ]);
  assert.deepEqual(calls[2], ["npm", ["install", "-g", "frob@1.2.3"]]);
  assert.deepEqual(calls[3], ["frob", ["install"]]);
});

test("skips Vercel system dependencies when explicitly disabled", async () => {
  const calls = [];
  const sandbox = {
    async runCommand(command, args) {
      calls.push([command, args]);
      return commandResult();
    },
  };

  await installFrobInVercelSandbox(sandbox, {
    installSpec: "frob@1.2.3",
    installSystemDependencies: false,
  });

  assert.deepEqual(calls, [
    ["npm", ["install", "-g", "frob@1.2.3"]],
    ["frob", ["install"]],
  ]);
});

test("rejects invalid Vercel system dependency names", async () => {
  const calls = [];
  const sandbox = {
    async runCommand(command, args) {
      calls.push([command, args]);
      return commandResult();
    },
  };

  await assert.rejects(
    () =>
      installFrobInVercelSandbox(sandbox, {
        systemDependencies: ["nss; touch /tmp/pwned"],
      }),
    /Invalid system dependency name/,
  );
  assert.deepEqual(calls, []);
});

test("stops a fresh Vercel sandbox when bootstrap fails", async () => {
  let stopped = false;
  const sandbox = {
    async runCommand() {
      return commandResult("", "install failed", 1);
    },
    async snapshot() {
      return { snapshotId: "snap" };
    },
    async stop() {
      stopped = true;
    },
  };

  await assert.rejects(
    () =>
      createFrobSandbox({
        Sandbox: { async create() { return sandbox; } },
        env: {},
        install: { systemDependencies: [] },
      }),
    /install failed/,
  );
  assert.equal(stopped, true);
});

test("withFrobSandbox stops the sandbox", async () => {
  let stopped = false;
  const sandbox = {
    async runCommand() {
      return commandResult();
    },
    async snapshot() {
      return { snapshotId: "snap" };
    },
    async stop() {
      stopped = true;
    },
  };

  const value = await withFrobSandbox(async () => 42, {
    Sandbox: { async create() { return sandbox; } },
    bootstrap: false,
    env: {},
  });

  assert.equal(value, 42);
  assert.equal(stopped, true);
});

test("withFrobSandbox preserves callback failure when stop fails", async () => {
  const sandbox = {
    async runCommand() {
      return commandResult();
    },
    async snapshot() {
      return { snapshotId: "snap" };
    },
    async stop() {
      throw new Error("stop failed");
    },
  };

  await assert.rejects(
    () =>
      withFrobSandbox(
        async () => {
          throw new Error("work failed");
        },
        {
          Sandbox: { async create() { return sandbox; } },
          bootstrap: false,
          env: {},
        },
      ),
    /work failed/,
  );
});

test("withFrobSandbox surfaces stop failure after success", async () => {
  const sandbox = {
    async runCommand() {
      return commandResult();
    },
    async snapshot() {
      return { snapshotId: "snap" };
    },
    async stop() {
      throw new Error("stop failed");
    },
  };

  await assert.rejects(
    () =>
      withFrobSandbox(async () => 42, {
        Sandbox: { async create() { return sandbox; } },
        bootstrap: false,
        env: {},
      }),
    /stop failed/,
  );
});

test("creates a Vercel sandbox snapshot", async () => {
  const sandbox = {
    async runCommand() {
      return commandResult();
    },
    async snapshot() {
      return { snapshotId: "snap_123" };
    },
    async stop() {},
  };

  const snapshotId = await createFrobSnapshot({
    Sandbox: { async create() { return sandbox; } },
    env: {},
    install: { systemDependencies: [] },
  });

  assert.equal(snapshotId, "snap_123");
});

test("createFrobSnapshot preserves snapshot failure when stop fails", async () => {
  const sandbox = {
    async runCommand() {
      return commandResult();
    },
    async snapshot() {
      throw new Error("snapshot failed");
    },
    async stop() {
      throw new Error("stop failed");
    },
  };

  await assert.rejects(
    () =>
      createFrobSnapshot({
        Sandbox: { async create() { return sandbox; } },
        env: {},
        install: { systemDependencies: [] },
      }),
    /snapshot failed/,
  );
});
