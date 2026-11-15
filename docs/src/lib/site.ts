export const siteUrl = "https://github.com/soulcloude00/frob";
export const siteName = "frob";
export const siteDescription = "Browser automation CLI for AI agents";

export function canonicalUrlFor(href: string): string {
  return `${siteUrl}${href === "/" ? "" : href}`;
}

export function isPreviewDeployment(): boolean {
  return Boolean(
    process.env.VERCEL_ENV && process.env.VERCEL_ENV !== "production",
  );
}
