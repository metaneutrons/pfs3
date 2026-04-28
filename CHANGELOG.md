# Changelog

## [0.1.2](https://github.com/metaneutrons/pfs3/compare/v0.1.1...v0.1.2) (2026-04-28)


### Bug Fixes

* **ci:** trigger release workflow from release-please on new release ([9b0a2b5](https://github.com/metaneutrons/pfs3/commit/9b0a2b5cbe533429b89e9bbc6684b74f03e7da36))
* **libpfs3:** add bounds checks in dir entry extra-fields walk ([2d38e02](https://github.com/metaneutrons/pfs3/commit/2d38e022da1b4eb0faae8136b0ece496e452b171))
* **libpfs3:** add checked arithmetic in alloc_data_blocks ([0a74581](https://github.com/metaneutrons/pfs3/commit/0a745817536651dfcf1123bb897f4eb17566e876))
* **libpfs3:** cap read_file_data allocation against corrupt size fields ([d9dc4f6](https://github.com/metaneutrons/pfs3/commit/d9dc4f681cce2fd8159e8c5c2c176a94a833406b))
* **libpfs3:** read rdb_highblock from RDSK header ([2cd765a](https://github.com/metaneutrons/pfs3/commit/2cd765ace8dbfb151c1b53ce8573654c3069b18c))
* **libpfs3:** replace .unwrap() with .unwrap_or_default() on SystemTime ([5d9f15d](https://github.com/metaneutrons/pfs3/commit/5d9f15d84a3262479aeb274979af19ae6981e721))
* **pfs3-fuse:** preserve write buffer on flush error in release ([497ebdc](https://github.com/metaneutrons/pfs3/commit/497ebdcaaacd5033d815662948d54b8c887552f2))

## [0.1.1](https://github.com/metaneutrons/pfs3/compare/v0.1.0...v0.1.1) (2026-04-14)


### Bug Fixes

* **ci:** match release workflow tag pattern to release-please (v* not pfs3-v*) ([29f29bc](https://github.com/metaneutrons/pfs3/commit/29f29bc2194bc4df682ad27d852265930ac120c0))
* **ci:** update AUR deploy action to v4.1.2 (fixes su entrypoint) ([e09adf4](https://github.com/metaneutrons/pfs3/commit/e09adf4f6dcb67c97f4c980320888960a3b8cbbd))

## 0.1.0 (2026-04-14)


### Features

* initial release — PFS3 tools, FUSE driver, and libpfs3 library ([4e7245b](https://github.com/metaneutrons/pfs3/commit/4e7245be291d31fa5017fa8cbd9e6d26a08f1e57))


### Bug Fixes

* **ci:** drop aarch64-linux FUSE cross-build, add fail-fast: false ([6dc12dc](https://github.com/metaneutrons/pfs3/commit/6dc12dc6c021a638e8b9c419ad464519995a41e3))
* **ci:** use native ARM runner for aarch64-linux FUSE build ([c81e142](https://github.com/metaneutrons/pfs3/commit/c81e14291c699bc103e344d57041036746579501))
* resolve clippy warnings and CI workflow heredoc syntax ([a2ea4a0](https://github.com/metaneutrons/pfs3/commit/a2ea4a0e79bff786ccb2ccc06ee7e09488683a9d))
