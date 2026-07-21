## 1. The flag and its config

- [ ] 1.1 Add `--provision-sdk` to kenn-dotnet's `index` command, default off,
  and `[language.csharp] provision_sdk` to kenn-config, threaded to the docker
  launcher. → verify: absent flag = today's behavior exactly (an unsatisfiable
  pin is a named failure, nothing installed).

## 2. Detect the SDK-resolution failure and the wanted version

- [ ] 2.1 Recognize the `hostfxr_resolve_sdk2` / "compatible .NET SDK was not
  found" failure from `OpenProjectAsync`, distinct from every other load error.
  → verify: a fabricated non-SDK load error does NOT take the install path.
- [ ] 2.2 Read the effective `global.json` for the failing project (walk up
  from the project dir, as MSBuild does) to get the pinned version and
  `rollForward`. `MsBuildBootstrap.FindSdkPin` already walks for this — reuse
  it. → verify: the version read for a `Src/**` project is `Src/global.json`'s,
  not the root's.

## 3. Install into the shared cache, atomically

- [ ] 3.1 Install the pinned SDK via the official `dotnet-install` script into
  `<cache>/<arch>/dotnet/<version>`, staged-and-renamed. → verify: an
  interrupted install leaves no directory a later run treats as complete.
- [ ] 3.2 Point the BuildHost's SDK search at the installed SDK and retry the
  load ONCE. A second failure for the same version is terminal and named (D5).
  → verify: after install, the project loads; a deliberately-corrupt install
  fails named, not looping.
- [ ] 3.3 Bound the install (timeout) and name every failure with the pin and
  its `global.json` path — never a raw script error, never a hang (D5). →
  verify: a pin naming a nonexistent version fails named and bounded.

## 4. Reuse and visibility

- [ ] 4.1 An already-installed version is reused, not re-downloaded. → verify: a
  second index of the same workspace does not re-install.
- [ ] 4.2 An SDK installed this way is listed by `kenn docker-cache` like any
  entrypoint-provisioned toolchain. → verify: it appears in the toolchain
  listing with its arch and version.

## 5. Verify the motivating case end-to-end

- [ ] 5.1 Index Newtonsoft.Json through the docker runtime with the flag on: its
  `Src/global.json` pins SDK 9.0.300, the container provisions SDK 10, and the
  flag installs 9.x and indexes. → verify: a real symbol count (host gives 945
  files / 20478 symbols), not 0. The container must NOT already have the pinned
  SDK — an install that finds it present proves only plumbing (D5 risk note).
- [ ] 5.2 With the flag OFF, the same repo still fails named and installs
  nothing. → verify: the strict default is intact.
