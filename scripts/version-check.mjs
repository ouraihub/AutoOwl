// Fails unless package.json, src-tauri/Cargo.toml and src-tauri/tauri.conf.json carry
// the same version, and (when a tag is given) the tag is exactly `v<version>`.
// Usage: node scripts/version-check.mjs [v1.2.3]
import { readFileSync } from 'node:fs';

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');

const versions = {
  'package.json': JSON.parse(read('package.json')).version,
  'src-tauri/tauri.conf.json': JSON.parse(read('src-tauri/tauri.conf.json')).version,
  // First `version = "..."` is the [package] one; dependency versions come later.
  'src-tauri/Cargo.toml': read('src-tauri/Cargo.toml').match(/^version\s*=\s*"([^"]+)"/m)?.[1],
};

const distinct = new Set(Object.values(versions));
if (distinct.size !== 1 || distinct.has(undefined)) {
  console.error('version mismatch:', versions);
  process.exit(1);
}
const [version] = distinct;

const tag = process.argv[2];
if (tag !== undefined && tag !== `v${version}`) {
  console.error(`tag ${tag} does not match version ${version} (expected v${version})`);
  process.exit(1);
}
console.log(`version ${version}${tag ? `, tag ${tag}` : ''} OK`);
