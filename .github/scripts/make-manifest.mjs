// 生成并签名 `manifest.json` —— 客户端把 GitHub 发布页当更新源时读的就是它。
//
// # 为什么需要它
//
// GitHub Release 本身给不了客户端要的三样东西：`build`（比新旧靠它，版本号只是给人看的）、
// 每个平台的 sha256、以及**离线签名**。客户端的流程是「先下这份小清单 → 验签 → 再下安装包」，
// 所以一份被改过的 manifest 连下载都不会开始。
//
// # 为什么用 JS 而不是 Rust
//
// 签名要与客户端的 libsm 互通。实测 `sm-crypto-v2` 的 `doSignature(..., {der:true, hash:true})`
// 产出的签名 libsm 验得过；**少了 `hash:true` 就验不过**（那一步做的是 SM3 + ZA）。
// 用 node 省掉在发版流水线里编一遍 Rust 的几分钟。
//
// ⚠️ 这是一处**跨实现依赖**：sm-crypto-v2 若在某次升级里改了默认 userId 或 DER 编码，
// 签出来的东西 libsm 就验不过，而这种失败要等用户装不上才暴露。因此：
//   1. 工作流里把版本钉死，不要用 ^ 或 latest；
//   2. 本脚本签完**当场自验**一遍；
//   3. 升级这个依赖时，必须重新跑一遍「JS 签 → ferric-sign verify」的互通验证。
//
// # 用法
//
//   node make-manifest.mjs <产物目录> <版本号> <构建号> [最低支持构建号]
//
// 私钥从环境变量 `FERRIC_RELEASE_KEY` 读，**绝不接受命令行参数** ——
// 命令行会进程列表可见，也会落进 CI 日志。
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import pkg from 'sm-crypto-v2';

const { sm2 } = pkg;
const [dir, version, buildStr, minSupportedStr = '0'] = process.argv.slice(2);
const key = process.env.FERRIC_RELEASE_KEY?.trim();

if (!dir || !version || !buildStr) {
  console.error('用法：node make-manifest.mjs <产物目录> <版本号> <构建号> [最低支持构建号]');
  process.exit(2);
}
if (!key) {
  console.error('缺少环境变量 FERRIC_RELEASE_KEY（发布私钥 hex）');
  process.exit(2);
}
const build = Number.parseInt(buildStr, 10);
const minSupported = Number.parseInt(minSupportedStr, 10) || 0;
if (!Number.isInteger(build) || build <= 0) {
  console.error(`构建号须为正整数，收到「${buildStr}」—— 客户端靠它比新旧`);
  process.exit(2);
}
if (minSupported > build) {
  console.error('最低支持构建号不能大于本次构建号，那会把用户困在一个不存在的版本上');
  process.exit(2);
}

/** 从产物文件名里认出平台与架构。命名规则见 release.yml 的 Collect artifacts 步。 */
const TARGETS = [
  { match: /-windows-x86_64-setup\.exe$/, platform: 'windows', arch: 'x86_64' },
  { match: /-windows-aarch64-setup\.exe$/, platform: 'windows', arch: 'aarch64' },
  { match: /-macos-x86_64\.dmg$/, platform: 'macos', arch: 'x86_64' },
  { match: /-macos-aarch64\.dmg$/, platform: 'macos', arch: 'aarch64' },
  { match: /-linux-x86_64\.deb$/, platform: 'linux', arch: 'x86_64' },
  { match: /-linux-aarch64\.deb$/, platform: 'linux', arch: 'aarch64' }
];

/** 与客户端 `release::signing_payload` **逐字节一致**。改这里必须同步改那边。 */
function signingPayload(v, b, platform, arch, sha256, size) {
  return Buffer.from(
    `ferric-update-v1\nversion=${v}\nbuild=${b}\nplatform=${platform}\narch=${arch}\n` +
      `sha256=${sha256}\nsize=${size}\n`
  );
}

const pubkey = sm2.getPublicKeyFromPrivateKey(key);
const artifacts = [];

for (const file of readdirSync(dir).sort()) {
  const t = TARGETS.find(x => x.match.test(file));
  // 每个「平台 + 架构」在 manifest 里只能有一条（客户端取第一条匹配的）。
  // Linux 选 .deb 而不是 .AppImage：客户端两者都收，但 .deb 交给系统软件安装器
  // 才是「安装」语义，与 Windows/macOS 一致。portable 版同理不进 manifest ——
  // 它是给「不想安装」的人手工下的，自动更新去覆盖一个绿色版没有意义。
  if (!t) continue;

  const bytes = readFileSync(join(dir, file));
  if (bytes.length === 0) {
    console.error(`${file} 是空文件`);
    process.exit(1);
  }
  const sha256 = createHash('sha256').update(bytes).digest('hex');
  const size = bytes.length;
  const ext = file.slice(file.lastIndexOf('.'));
  const payload = signingPayload(version, build, t.platform, t.arch, sha256, size);

  // der:true → DER 编码；hash:true → 先做 SM3 + ZA。两个都不能少，否则 libsm 验不过
  const signature = sm2.doSignature(new Uint8Array(payload), key, { der: true, hash: true });

  // 签完当场自验：与其让用户装不上才发现，不如在这里就红
  if (!sm2.doVerifySignature(new Uint8Array(payload), signature, pubkey, { der: true, hash: true })) {
    console.error(`${file} 自验未通过，已中止`);
    process.exit(1);
  }

  artifacts.push({ platform: t.platform, arch: t.arch, ext, file, sha256, size, signature });
  console.error(`已签名 ${t.platform}/${t.arch}  ${file}  ${size} 字节`);
}

if (artifacts.length === 0) {
  console.error(`${dir} 里没有可签名的安装包 —— 命名规则变了？`);
  process.exit(1);
}

const notes = process.env.FERRIC_RELEASE_NOTES ?? '';
const manifest = { version, build, notes, min_supported_build: minSupported, artifacts };
writeFileSync(join(dir, 'manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`);
console.error(`已写出 ${artifacts.length} 个平台的 manifest.json`);
// 公钥打到日志里，方便与客户端烘入的 FERRIC_RELEASE_PUBKEY 核对（公钥可以公开）
console.error(`本次使用的发布公钥：${pubkey}`);
