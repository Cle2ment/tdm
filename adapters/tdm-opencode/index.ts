// Root entry so the package also works as a directory-form plugin
// (.opencode/plugins/<name>/index.ts auto-discovery) and with loaders that
// resolve directories via index files rather than package.json main/exports.
export { default } from "./src/index";
