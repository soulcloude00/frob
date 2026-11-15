import { defineConfig } from "@vercel/geistdocs/config";
import { siteUrl } from "@/lib/site";

export const config = defineConfig({
  title: "frob",
  siteUrl,
  defaultLanguage: "en",
  logo: (
    <span
      className="font-medium tracking-tight"
      style={{ fontFamily: "var(--font-geist-pixel-square)" }}
    >
      frob
    </span>
  ),
  navbarActiveProduct: "frob",
  navbarBrand: "labs",
  github: {
    owner: "vercel-labs",
    repo: "frob",
    branch: "main",
    editPath: "docs/content/docs",
  },
  content: [
    { id: "docs", label: "Documentation", dir: "content/docs", route: "/" },
  ],
  nav: [
    { label: "Docs", href: "/" },
    {
      label: "npm",
      href: "https://www.npmjs.com/package/frob",
      external: true,
    },
  ],
  ai: { enabled: false },
  feedback: { enabled: false },
  language: { enabled: false },
  pageActions: { askAI: false, openInChat: false, copyPage: true },
  webmcp: { enabled: true },
});
