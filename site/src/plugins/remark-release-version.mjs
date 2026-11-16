// The docs show release archives by name (`kariz-v2.8.5-x86_64-linux.tar.gz`). On the site
// those names always carry the version being published, so a guide never tells anyone to
// download an older release than the newest one. A beta in Cargo.toml leaves them as written:
// the guides send people to the latest stable release.
const ARCHIVE = /kariz-v\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?(?=-(?:x86_64|aarch64|armv7)-linux)/g;

export function remarkReleaseVersion({ version }) {
  const stable = /^\d+\.\d+\.\d+$/.test(version ?? "");
  return (tree) => {
    if (!stable) return;
    const walk = (node) => {
      if ((node.type === "text" || node.type === "inlineCode" || node.type === "code") && typeof node.value === "string") {
        node.value = node.value.replace(ARCHIVE, `kariz-v${version}`);
      }
      if (node.children) node.children.forEach(walk);
    };
    walk(tree);
  };
}
