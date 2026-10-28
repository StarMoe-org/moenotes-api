# Protocol Provenance

`descriptors.pb` is a recovered FileDescriptorSet from the Android
`com.bilibili.sirius` **1.1.0 (versionCode 86)** sample (Unity 6000.3.12f1).
It contains 125 files, 57 services and 276 unary methods, including Google
protobuf descriptors.

SHA-256: `53eac05bf92354df463770d85ed09406b2964ff3b7ee281c2d38ea0d9e6290f0`

Recovery: each `App.Protobuf.*Reflection$$.cctor` was disassembled from the
recovered `libil2cpp.so`; the base64 fragments it loads from metadata-usage slots
(`R_AARCH64_RELATIVE` addends) were concatenated and parsed as
`FileDescriptorProto`. 125 of 125 constructors parsed, with no failures. The
reflected set alone is 125 files / 57 services / 274 methods
(SHA-256 `72dd56322e64dbcde1f4134bf29238b6bf1351e28e4115310da87bc32c3f61ae`).

**Two methods are retained from the 1.0.1 sample** because the 1.1.0 client no
longer declares them, while this gateway still exposes one of them:

- `app.player.PlayerService/Whoami` (+ `WhoamiRequest`, `WhoamiResponse`)
- `app.membercard.MemberCardService/LinkSkillLevelUp` (no gateway route)

The resulting 276-method snapshot is therefore a compatibility union, not a pure
reflection dump of one build. Everything else is exactly the 1.1.0 reflection.
Whether a 1.1.0 server still answers `Whoami` is unverified.

The 1.1.0 sample is a **third-party re-signed APK** (debug/test certificate,
`MetaMiku.RSA`); its descriptors are used as protocol evidence, not as proof that
the official binary is byte-equivalent.

`jp-descriptors.pb` is independently recovered from Android
`com.bushiroad.sirius` 1.0.3 / 10050: 117 files, 50 services and 249 unary methods.
SHA-256: `5dd2127265b32eb3cfb6cdf60ed3bf34d9138faea7c768f8714adcc5ce827ce5`.
JP uses a separate runtime descriptor pool; it is never merged into the
international pool. Compatible query request types remain a shared facade,
with JP-specific service routing and response decoding. The JP snapshot predates
`GetChallengeMusicTotalRanking`, so that query is rejected for `jp`.

The snapshot is not an official published API specification. Its third-party
contents and generated protocol definitions are NOT claimed as original MIT
licensed work. Original build and reflection helpers are covered by the root
license. No credentials, account captures, master tables, executable samples,
or private analysis artifacts are included.

Cargo generates Rust message types directly from this snapshot with prost-build;
no proprietary tools, game installation, protoc or live server is required.
Future snapshot changes require a hash/provenance update and compatibility review.
