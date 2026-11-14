# Eidoryn dependencies and test artifacts

The emulator CPU, bus, software/wgpu GPU algorithms and shaders, DMA, disc reader, CD-ROM controller, EXE
loader, CUE parser, GTE, MDEC, SIO/digital pad, memory-card protocol/persistence,
SPU voices/ADSR/reverb, XA/CDDA decoding, timers and MIPS test programs are implemented locally. No third-party emulator
core, emulator adapter, or vendored emulator source is used.
The application now depends on local `eidoryn-ps1` (hardware) and `eidoryn-host`
(audio, filters, themes) crates. With optional features disabled, `eidoryn-host`
uses only the Rust standard library. `eidoryn-ps1` uses smallvec 1.16.5
(MIT/Apache-2.0, `union` feature) for private CD-ROM command parameters and delayed
responses. The inline capacities are 16 and 8 bytes; larger values can still use
heap storage. Existing sector and FIFO buffers retain their reusable capacities.

The optional `dx12`, `metal`, `vulkan` and `opengl` features use wgpu 30.0.2 for safe resource and compute
submission APIs, pollster 1.0.1 for initialization, and bytemuck 1.25.2 for safe
integer-slice byte views. These are general-purpose libraries; PS1 command
encoding, dependency tracking, rasterization and scanout are implemented here.
OpenGL uses wgpu's `gles` backend (native WGL on Windows), including glow and
glutin_wgl_sys bindings; the emulator introduces no unsafe OpenGL calls.
API reference: https://docs.rs/wgpu/30.0.2/wgpu/ . The default build includes `desktop`, `hardware` (all four backends) and `jit`;
runtime CPU/renderer selections are independent.

Cargo.lock resolves gpu-allocator 0.28.0's Windows API dependency to windows 0.62.2,
matching wgpu-hal's DirectX 12 interface types. Other dependencies may retain older
Windows API versions; these cannot be substituted across the allocator boundary.

The desktop frontend uses GPUI, image, serde and serde_json (theme manifests),
and CPAL 0.18.2 for host audio-device access (not PS1 sound emulation).
Their transitive dependencies and exact versions
are recorded in Cargo.lock and retain their own licenses. This project keeps
its existing GPL-3.0-only package license; LICENSE-MIT preserves the original
prototype's notice.

GPUI and `gpui_platform` (Apache-2.0) follow the official Zed `main` branch.
Cargo.lock pins both to the same tested commit, currently
[`0cfc4a483dfab6ca171df0ceedaaa2a9f7954d0e`](https://github.com/zed-industries/zed/commit/0cfc4a483dfab6ca171df0ceedaaa2a9f7954d0e)
(the branch head checked on 2026-11-14). This revision contains
[upstream PR #42227](https://github.com/zed-industries/zed/pull/42227), which fixes
Windows file-drop crashes by passing `STGMEDIUM.hGlobal` directly as an `HDROP`.
The complete Git revision and Cargo.lock fix the dependency source; its package
version label alone does not identify this fix. No GPUI source is vendored or
patched locally. The upstream copyright and license text are preserved below.
This document is included in release packages.

The root patch table carries the relevant fixes from that upstream workspace:
`async-task` preserves task-waker identity, `calloop` fixes Linux event-loop
fairness and reentrancy, and `khronos-egl` handles failed EGL calls that report
`EGL_SUCCESS`. Each is pinned to its upstream Git revision in Cargo.toml and
Cargo.lock. The former GPUI compatibility overrides for `libc` and
`proc-macro-error2` are no longer needed.

The optional `jit` feature uses the local `eidoryn-jit-runtime` crate, an original
MIPS-integer-to-x86-64 emitter and safe native execution API. Its only external
library is memmap2 0.9.12 for anonymous RW-to-RX executable mappings (MIT/Apache-2.0).
No emulator recompiler or third-party code generator is copied or linked.
The main emulator keeps `unsafe_code = forbid`; the isolated native-call boundary
is documented and tested in crates/jit-runtime/SAFETY.md.

## Hardware references

The implementation refers to the PSX-SPX hardware research documentation:

- https://psx-spx.consoledev.net/cdromdrive/
- https://psx-spx.consoledev.net/cdromformat/
- https://psx-spx.consoledev.net/dmachannels/
- https://psx-spx.consoledev.net/cdromfileformats/
- https://psx-spx.consoledev.net/graphicsprocessingunitgpu/
- https://psx-spx.consoledev.net/soundprocessingunitspu/
- https://psx-spx.consoledev.net/timers/
- https://psx-spx.consoledev.net/expansionportpio/
- https://psx-spx.consoledev.net/geometrytransformationenginegte/
- https://psx-spx.consoledev.net/macroblockdecodermdec/
- https://psx-spx.consoledev.net/serialinterfacessio/
- https://psx-spx.consoledev.net/controllersandmemorycards/

These are behavior and register references, not source dependencies.
The 512-entry SPU Gaussian ROM table and seven XA zigzag filter tables are
hardware coefficients transcribed from these documents. The surrounding
decoder, interpolation, mixer and device state machines are implemented here;
no CoffeePSX or other emulator implementation was copied or linked.

## Distributed assets

`assets/eidoryn.png` is an AI-generated icon created for this project. It is embedded
in the GPUI header. Test programs and disc generation scripts are original.
No game images, proprietary BIOS, save files, gameplay captures, or local testing
artifacts are distributed with the application.

## GPUI license

<details>
<summary>Copyright notice and Apache License 2.0</summary>

```text
Copyright 2022 - 2025 Zed Industries, Inc.


   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at


       http://www.apache.org/licenses/LICENSE-2.0


   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.




Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/


   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION


   1. Definitions.


      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.


      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.


      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.


      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.


      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.


      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.


      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).


      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.


      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."


      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.


   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.


   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.


   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:


      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and


      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and


      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and


      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.


      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.


   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.


   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.


   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.


   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.


   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.


   END OF TERMS AND CONDITIONS
```

</details>
