// Standalone site for dreamd, served at https://fongo.dev/dreamd by a Cloudflare
// Worker (static assets) on the zone route `fongo.dev/dreamd*`.
// `outDir` nests the build under dist/dreamd so the Worker's 1:1 path→asset
// mapping lines up with the route prefix (Astro's `base` only prefixes URLs).
import { defineConfig, fontProviders } from "astro/config";

export default defineConfig({
  site: "https://fongo.dev",
  base: "/dreamd",
  outDir: "./dist/dreamd",
  // Slashless URLs are canonical, matching the Worker's drop-trailing-slash.
  trailingSlash: "never",
  experimental: {
    fonts: [
      {
        provider: fontProviders.google(),
        name: "Spectral",
        cssVariable: "--font-display",
        // Only 500 ships: headings are 500 and the wordmark is italic 500.
        // Body prose is the system sans stack, so 400 would be dead weight.
        weights: [500],
        styles: ["normal", "italic"],
        subsets: ["latin"],
        fallbacks: ["Georgia", "Cambria", "Times New Roman", "serif"],
      },
    ],
  },
});
