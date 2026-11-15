import { existsSync } from "node:fs";
import process from "node:process";
import { runFrobCommand, withFrobSandbox } from "@frob/sandbox/vercel";

if (existsSync(".env.local") && typeof process.loadEnvFile === "function") {
  process.loadEnvFile(".env.local");
}

const url = process.argv[2] ?? "https://example.com";

const snapshot = await withFrobSandbox(async (sandbox) => {
  await runFrobCommand(sandbox, ["open", url]);
  const result = await runFrobCommand(sandbox, ["snapshot", "-i", "-c"], {
    json: false,
  });
  await runFrobCommand(sandbox, ["close"], { json: false });
  return result.stdout;
});

console.log(snapshot);
