import browser from "@frob/eve";

// Mounts the frob tool set under the `browser` namespace:
// browser__open, browser__snapshot, browser__click, browser__fill, ...
export default browser({
  // The sandbox bootstrap in agent/sandbox.ts pre-installs frob, so
  // tools never pay the install cost on first use. autoInstall stays on as a
  // fallback for sandboxes created without the bootstrap.
  // allowedDomains: ["example.com", "*.example.com"],
  // maxOutputChars: 50_000,
});
