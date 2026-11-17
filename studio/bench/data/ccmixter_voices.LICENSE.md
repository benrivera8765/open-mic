# ccmixter_voices.json - source and licence

`ccmixter_voices.json` in this folder is an unmodified copy of
`musetric_toolkit/distill/data/ccmixter_voices.json` from https://github.com/musetric/lab
(git blob `f18a219a1a3e18705b866493a5e69a8f484c8c3b`, 5,400 bytes; identical to the
upstream file as checked on 2026-11-17). It lists ccMixter uploads whose lead and backing
vocals were published as separate files; `fetch_eval.py` uses it to build the
lead/backing evaluation set. The audio itself is never committed (each upload keeps its
uploader's Creative Commons licence, see `$JK_BUILD/eval/ccmixter_voices/LICENSE-EVAL.md`).

The file is distributed under the MIT licence of musetric/lab:

```
MIT License

Copyright (c) 2025 Vladlen Popelenkov

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
