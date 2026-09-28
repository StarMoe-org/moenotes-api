# Protocol Provenance

`descriptors.pb` is a recovered FileDescriptorSet from the Android
`com.bilibili.sirius` 1.0.1 sample (Unity 6000.3.12f1). It contains 123 files,
56 services and 261 unary methods, including Google protobuf descriptors.

SHA-256: `d07bd23de3553f71caec0830668bf4f5196fe33575a265ed0489f73ffb60cf20`

`jp-descriptors.pb` is independently recovered from Android
`com.bushiroad.sirius` 1.0.3 / 10050: 117 files, 50 services and 249 unary methods.
SHA-256: `5dd2127265b32eb3cfb6cdf60ed3bf34d9138faea7c768f8714adcc5ce827ce5`.
JP uses a separate runtime descriptor pool; it is never merged into the
international pool. Compatible query request types remain a shared facade,
with JP-specific service routing and response decoding.

The snapshot is not an official published API specification. Its third-party
contents and generated protocol definitions are NOT claimed as original MIT
licensed work. Original build and reflection helpers are covered by the root
license. No credentials, account captures, master tables, executable samples,
or private analysis artifacts are included.

Cargo generates Rust message types directly from this snapshot with prost-build;
no proprietary tools, game installation, protoc or live server is required.
Future snapshot changes require a hash/provenance update and compatibility review.
