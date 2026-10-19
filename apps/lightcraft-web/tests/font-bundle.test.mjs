import test from 'node:test';
import {verifyFontAssets, verifyBundle} from './font-bundle.mjs';
const fixture = process.env.LIGHTCRAFT_WEB_FONT_FIXTURE;
const bundle = process.env.LIGHTCRAFT_WEB_FONT_BUNDLE;
test('exact generated font, descriptor and licence/copyright notices', {skip: !fixture}, async () => verifyFontAssets(fixture));
test('every optimized bundle/packaged file stays within 25 MiB', {skip: !bundle}, async () => verifyBundle(bundle));
