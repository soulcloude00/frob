import { frobRevalidationKey, installFrob } from "@frob/eve/sandbox";
import { defineSandbox } from "eve/sandbox";
import { vercel } from "eve/sandbox/vercel";

export default defineSandbox({
  backend: vercel({ resources: { vcpus: 2 } }),
  revalidationKey: () => frobRevalidationKey(),
  async bootstrap({ use }) {
    const sandbox = await use();
    await installFrob(sandbox);
  },
});
