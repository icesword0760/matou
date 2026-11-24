// Test-harness path redirection only; frozen baseline and renderer stay unchanged.
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const asar = require(process.argv[2]);
(async () => {
  const root = process.argv[3];
  const baselineApp = process.argv[4] || path.join(root, 'baseline/MatouReference.app');
  const referenceApp = process.argv[5] || path.join(root, 'reference/MatouParityReference.app');
  const original = path.join(baselineApp, 'Contents/Resources/app.asar');
  const destination = path.join(referenceApp, 'Contents/Resources/app.asar');
  const extracted = path.join(path.dirname(referenceApp), 'home-harness-source');
  asar.extractAll(original, extracted);
  const main = path.join(extracted, 'out/main/index.js');
  const before = fs.readFileSync(main, 'utf8');
  const anchor = 'applyApplicationBrand(app, APP_DISPLAY_NAME);';
  if (before.split(anchor).length !== 2) throw new Error('Reference entry changed; inspect before preparing harness');
  fs.writeFileSync(main, before.replace(anchor,
    'app.setPath("home", process.env.MATOU_REFERENCE_HOME);\n' + anchor));
  await asar.createPackage(extracted, destination);
  const hash = crypto.createHash('sha256').update(asar.getRawHeader(destination).headerString).digest('hex');
  fs.writeFileSync(path.join(path.dirname(referenceApp), 'home-harness.json'), JSON.stringify({
    changed_file: 'out/main/index.js',
    added_line: 'app.setPath("home", process.env.MATOU_REFERENCE_HOME);',
    reason: 'Electron app.getPath(home) ignores isolated HOME on this Mac; contain Finder service writes',
    renderer_changed: false, header_sha256: hash
  }, null, 2));
})().catch(e => { console.error(e); process.exitCode=1; });
