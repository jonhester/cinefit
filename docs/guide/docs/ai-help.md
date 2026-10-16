---
title: "Getting help from an AI"
description: "How to point ChatGPT, Claude or another AI assistant at the cinefit guide with llms.txt, and what to paste so it can help you set up or troubleshoot."
---

# Getting help from an AI

This guide is also published in a form AI assistants read easily:

- <a href="/llms.txt"><code>llms.txt</code></a>: an index of every page with a one-line summary.
- <a href="/llms-full.txt"><code>llms-full.txt</code></a>: the whole guide as one text file.

## How to ask

1. Give the assistant the guide: paste the link to `llms-full.txt`, or download the file and attach it.
2. Say what you have: the GPU, the capture card, the display and its native resolution, the source, and what is between them.
3. Paste what cinefit says. The most useful are:

```sh
cinefit --version
cinefit probe
cinefit display
cinefit settings
journalctl -u cinefit -n 100 --no-pager
```

For example:

```text
Using the cinefit guide at <link to llms-full.txt>: I have an RX 7600, a
DeckLink Mini Recorder 4K and a 3840x2160 projector on a 2.40 screen. The
slate says "Can't show this format". Here is cinefit probe and the journal: ...
```

::: callout warning "Check what it tells you"
An assistant can be confidently wrong, especially about root commands and EDID changes. Before running anything with `sudo`, or anything that changes the splitter's or capture card's EDID, check it against this guide. The [Settings](/settings/) is generated from cinefit's own code and is the authority on setting names and values.
:::

Before pasting logs or the settings file in public, remove anything private: addresses on your network, and the Apple TV's titles.
