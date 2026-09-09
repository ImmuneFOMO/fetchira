# Local CLI

[Home](../README.md) · [Setup and agent integration](setup.md) · [Configuration](configuration.md) · [Hosted](hosted.md)

The six one-shot commands use the same router as MCP. Configure at least one local provider first:

```sh
fetchira add serper --key 'YOUR_KEY'
fetchira search "latest Rust release"
fetchira read "https://example.com"
```

## Commands

The command forms are:

```text
fetchira search QUERY... [--provider P] [--max N] [--session S] [--model M] [--mode M] [--topic T] [--recency R] [--domain D]... [--file PATH]...
fetchira read URL [--provider P] [--mode M]
fetchira deep_research QUERY... [--provider P] [--max N] [--session S] [--model M] [--mode M] [--topic T] [--recency R] [--domain D]... [--file PATH]... [--depth standard|deep]
fetchira browser URL
fetchira create_image PROMPT... [--provider P] [--path DEST] [--session S] [--file PATH]...
fetchira usage [PROVIDER]
```

CLI calls use local accounts and reject a configured remote endpoint. They print provider text to
stdout, diagnostics to stderr, and append the same `⟦session: …⟧` footer as MCP. `deep_research`
also accepts `dr`; positional query and prompt words are joined, flags may be mixed with them,
and `--` ends flag parsing. Repeat `--domain` and `--file`; prefix an excluded domain with `-`
(for example `--domain -reddit.com`). Relative `--file` and `--path` values resolve from the
current directory. Only a ChatGPT research/image poll session permits omitted text:

```sh
fetchira search "follow-up question" --session 'TOKEN'
fetchira deep_research "start" --session 'PLAN_TOKEN'
fetchira deep_research --session 'CHATGPT_RESEARCH_POLL_TOKEN'
fetchira create_image --session 'CHATGPT_IMAGE_POLL_TOKEN'
```

`usage` no longer aliases `list`; use `list` or `accounts` for the human account table.
A completed image prints its saved absolute path; a pending research or image
poll prints its status and session so the next call can continue it.


## Provider options

Use `fetchira usage PROVIDER` for that provider's supported modes, models and examples. The
`model` option selects the web provider's model; it does not select the agent model running
this command. Omit it to use the provider default. Quotas and available models can change.

```sh
fetchira search "battery recycling" --provider serper --max 5 --topic news
fetchira search "summarize this file" --provider grok_web --file ./report.pdf
fetchira read "https://example.com" --provider steel --mode screenshot
fetchira usage chatgpt_web
```

`read --provider firecrawl --mode extract` returns single-page structured JSON. Its `crawl`
mode only starts a provider job and returns the job ID; Fetchira does not poll crawl jobs.

`browser` returns page content from Steel. It does not control your signed-in browser or
perform arbitrary clicks. `read` can fall back to browser rendering when content is empty.

## Sessions and research

Copy only the token inside `⟦session: TOKEN — …⟧` into `--session`, preserving every character.
It identifies the provider and account; it overrides `--provider`. Quote it because tokens
can contain shell punctuation.

Gemini and ChatGPT research first return a plan. Review it, send a refinement with the same
session if needed, then send `start`. Gemini usually returns its report in that call;
ChatGPT may return a polling session. Use the newest returned token for each poll. A pending
response is not a completed report or image.

## Output and errors

Success exits `0` and prints text to stdout. Bad arguments, unavailable accounts, provider
errors and image-write errors exit `1` and print diagnostics to stderr. Use
`fetchira COMMAND --help` for command-specific usage. There is no JSON output mode.

Completed images, screenshots and PDFs are saved under the Fetchira home `images/` directory
unless `create_image --path DEST` chooses a destination. An explicit existing destination
is overwritten. The CLI prints `saved: /absolute/path` and never emits base64 image bytes.

The `--file` option attaches local files only to providers that support uploads. It does not
upload files to hosted Fetchira. Once a remote endpoint is configured, all six commands
reject that configuration; use MCP, `fetchira remote disconnect`, or a separate local
`FETCHIRA_HOME`.
