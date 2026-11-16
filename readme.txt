SkillStar Installation Guide
===========================

Version 1.2.0
-------------
Skill packages for Claude, Agent Skills (Codex, Gemini CLI, Copilot,
Cursor), ChatGPT, Gemini, DeepSeek and RAG; questions answered from the
documents with clickable [n] citations; OCR language per document and
re-OCR. See whatsnew.txt for the release history.

The main-window Donate button appears after 10 minutes of foreground
use. The timer pauses while the window is unfocused or hidden and
resets each time SkillStar starts. You can also donate from the About
dialog at any time.

Download
--------
On the GitHub Actions page for a successful "Build installers" run,
download the ZIP for your platform from Artifacts and extract it.

Windows x64 and ARM64
---------------------
1. Open Settings > System > About and check System type.
2. For an x64 processor, choose the NSIS .exe from the
   SkillStar-x86_64-pc-windows-msvc artifact. For an ARM processor, choose
   the .exe from SkillStar-aarch64-pc-windows-msvc.
3. Run the SkillStar .exe installer. If the WebView2 Runtime is missing,
   the installer downloads it, which requires an internet connection.
   SmartScreen may warn about the unsigned installer. If you have
   verified the download source, select "More info" and then "Run anyway"
   if Windows offers that option.
4. Open SkillStar from the Start menu.

macOS Apple Silicon (ARM64) and Intel (x64)
------------------------------------------
1. Open Apple menu > About This Mac and check the chip or processor.
   For an Apple M-series chip, choose the .dmg from
   SkillStar-aarch64-apple-darwin. For an Intel processor, choose the .dmg
   from SkillStar-x86_64-apple-darwin.
2. Open the .dmg, drag SkillStar.app into Applications, and eject the disk
   image.
3. Open SkillStar from Applications. If Gatekeeper blocks the app because
   the developer cannot be verified, check the download source. After
   the first launch attempt, open System Settings > Privacy & Security,
   choose "Open Anyway" if available, and confirm.
   Apple guide: https://support.apple.com/en-ie/102445
4. If macOS says that "SkillStar is damaged and can't be opened", or
   "Open Anyway" does not appear, remove the quarantine flag. The app is
   not signed by Apple, so macOS blocks it. Run this in Terminal after
   copying the app to Applications:

     xattr -dr com.apple.quarantine /Applications/SkillStar.app

   If it still does not open, clear all extended attributes:

     xattr -cr /Applications/SkillStar.app

   Repeat this after installing a new version, including one installed by
   the self-update.

Linux x64
---------
Run "uname -m" and confirm that it reports x86_64. The
SkillStar-x86_64-unknown-linux-gnu artifact contains three formats.
Run the commands from the download folder and replace the example
filenames with the actual filenames.

Debian or Ubuntu (.deb)
  sudo apt update
  sudo apt install ./SkillStar_amd64.deb

Fedora or another DNF-based distribution (.rpm)
  sudo dnf install ./SkillStar.x86_64.rpm

AppImage
  chmod +x ./SkillStar_amd64.AppImage
  ./SkillStar_amd64.AppImage

  If you get a FUSE error, install your distribution's FUSE 2
  compatibility package (libfuse2 on Ubuntu 22.04 or libfuse2t64 on
  Ubuntu 24.04).

Where the data and settings are
-------------------------------
The skills, documents and extracted page texts are in a local SQLite
database (skillstar.db) in the app data folder (Windows:
%APPDATA%\hu.skillstar.app). Settings and API keys are in settings.json
in the same folder. The original files are never changed. The built
packages go to Documents/SkillStar (changeable in Settings > Skills).
Use About > Settings folder to open it.

PDF and OCR
-----------
PDFs are read with PDFium; the installers include it. If it is missing,
SkillStar offers to download it. Pages without a usable text layer are
read with OCR: on Windows with the built-in Windows OCR (add more
languages in Settings > Time & language > Language & region: install the
language with its optical character recognition feature), elsewhere, or
if you prefer it, with Tesseract (install it and set it in Settings >
Extraction). An interrupted OCR run continues where it stopped.

Using the packages
------------------
Choose the formats on the skill page or above the package preview.
Every format goes into its own folder of the output folder, each with
a README.md that explains the steps:
- claude/: Claude Code plugin folder, <name>.plugin and
  <name>-skill.zip (claude.ai > Settings > Capabilities > Skills);
- agents/: Agent Skills folder + ZIP (OpenAI Codex, Gemini CLI, GitHub
  Copilot, Cursor);
- chatgpt/: instructions.md + knowledge files for a Custom GPT or a
  ChatGPT Project;
- gemini/: instructions.md + knowledge files for a Gem;
- deepseek/: system prompt + the text cut into context-sized parts;
- rag/: chunks.jsonl, documents.json and a system prompt for any LLM
  and vector store.
