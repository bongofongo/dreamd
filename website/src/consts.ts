export const APP_NAME = "dreamd";
export const EMAIL = "fongo02@proton.me";
export const PERSONAL_SITE = "https://fongo.dev";
export const REPO_URL = "https://github.com/bongofongo/dreamd";
export const LICENSE_NAME = "Apache 2.0";
export const LICENSE_URL = "https://github.com/bongofongo/dreamd/blob/main/LICENSE";
export const VERSION = "0.3.0";
export const RELEASES_URL = `${REPO_URL}/releases/latest`;
export const INSTALL_URL =
  "https://raw.githubusercontent.com/bongofongo/dreamd/main/packaging/install.sh";
// The cask lives in ../packaging/cask.rb.tmpl and is published to the tap by the
// release workflow. Keep this token identical to the one in that file's header.
export const BREW_CASK = "bongofongo/tap/dreamd";

export const TAGLINE = "Read your repo like a book. Then ask it questions.";
export const SUBLINE =
  "A desktop markdown reader for a tmux, Neovim, and Claude Code workflow — typography freed from the terminal, and a way to hand what you read straight to your agent.";

// import.meta.env.BASE_URL is '/dreamd'; normalize so href('/gallery') is safe either way.
const BASE = import.meta.env.BASE_URL.replace(/\/$/, "");
export const href = (path: string) => `${BASE}${path === "/" ? "/" : path}`;
