# Privacy

Zero Use Computer (the `computer-use-mcp` program, its plugin and its
launcher) runs on your computer. It has no account, no server of its own,
and collects no analytics or telemetry. This page lists everything it reads,
keeps and sends.

## What it reads

To do what your agent asks, it reads the windows of the apps on your
desktop: their accessibility tree (names, roles and values of buttons, text
fields and the like), screenshots of them, and text read off the screen.
Passwords and card numbers are masked before anything is passed on
(`[privacy]` in the [guide](docs/GUIDE.md)), and the bundled security skill
sets which apps the agent may touch.

## Where that goes

- **To your agent, through the client you run it in** (Claude Code,
  Cowork, Claude Desktop, Codex, ...). The program answers the client over
  stdio (or a local HTTP port you turn on); what the client then sends to
  its model is governed by that client's and that model provider's
  policies, not by this program.
- **Nowhere else by default.** If you configure a *decision model*
  (`[decision]`, off unless you set a provider), the program sends it the
  small questions it delegates (for example a list of element names, or
  text from the screen) at the address and with the key you gave. Agents
  on one desktop can send each other notes only if you let them; those stay
  on your computer.

## What it sends to GitHub

- **Updates** (`[update]`, on by default; off with `enabled = false`): every
  12 hours or so the program asks GitHub's API about the latest release of
  `mhrsdev/zero-use-computer` and, when there is a newer one, downloads it
  from GitHub. The request carries the program's version in its
  User-Agent. Nothing about you or your screen is sent.
- **The plugin's launcher**, the first time it runs or when the plugin is
  newer than the program, downloads the program's release zip and its
  `.sha256` file from the same repository's releases on GitHub.

GitHub sees these requests as it sees any download (your IP address, the
time); see [GitHub's privacy statement](https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement).

## What it keeps on your computer

In `~/.computer-use` (or `$COMPUTER_USE_HOME`): the program, its settings
file, logs (including the launcher's), downloaded updates, and, if you
turn it on (`[audit]`), a log of the tools agents called (metadata only, no
values typed). Nothing there is sent anywhere. Delete the
folder to remove it all.

## Questions

Open an issue: https://github.com/mhrsdev/zero-use-computer/issues
