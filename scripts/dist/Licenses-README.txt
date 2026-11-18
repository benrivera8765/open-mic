Licences
========

@NAME@ is free software under the GNU General Public License, version 3 or (at your
option) any later version: LICENSE.txt. Its source code is the project's repository.

  LICENSE.txt               GPL-3.0 (the app, the Studio's own code, Rubber Band)
  THIRD_PARTY_LICENSES.md   every library, font, runtime and binary the apps include,
                            with its licence and what that licence requires
  MODELS.md                 every Studio model, where it comes from, its licence and
                            whether it allows commercial use
  fonts\                    the SIL Open Font License of the three bundled fonts
  python\                   licence texts of Studio packages whose files ship none
  third-party\              the licence and notice files of every library compiled into
                            the apps (Rust crates), of the user interface's packages and
                            of those Studio packages; MISSING.txt lists the few that
                            publish none and what is relied on instead

More licence texts travel inside the Studio itself:
Windows\Studio\python\Lib\site-packages\*.dist-info\ (one folder per Python package),
including the NVIDIA CUDA licence agreements for the GPU libraries.

The Windows Studio includes NVIDIA's CUDA libraries, the Microsoft Visual C++ runtime
and the Intel OpenMP runtime. Their licences allow them to be passed on with the
program, but they are not open source (THIRD_PARTY_LICENSES.md, section 1).
