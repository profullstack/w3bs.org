# w3bs ask: ask a published agent

```sh
w3bs ask business "What do I do, I can't make money???? HELP!"
```

`w3bs ask` puts a question to a published agent and streams its answer, followed by the web sources it was given. The model runs on the [c0mpute network](https://c0mpute.com): an [Infernet](https://infernetprotocol.com) node serves it, never your machine and never a paid hosted model.

```
w3bs: searching pit.world
w3bs: asking business on the c0mpute network
w3bs: profullstack-dev2 is answering
You can make money by ...

Sources:
  [1] I Have a Business Idea but No Money: 6 Steps to Start in 2026
      https://upmetrics.co/blog/start-a-business-without-money
  [2] ...

business answered with hf.co/profullstack/saas-business-strategist-agent-GGUF:latest on c0mpute node profullstack-dev2 (provider-8928d26f) in 19.5 s, first token 14.2 s
```

The answer and sources go to stdout; progress and the line naming the node go to stderr.

## Commands

```sh
w3bs ask --list                       # the agents, their aliases and models
w3bs ask business "How should I price my first plan?"
w3bs ask saas How should I price my first plan   # an alias; unquoted words work too
w3bs ask business "..." --no-search   # the model alone, no pit.world sources
w3bs ask business "..." --json        # one result object: answer, sources, node, job id, latency
w3bs ask                              # chat on a terminal (slash commands below)
```

Options: `--max-tokens N` (default: the agent's), `--timeout SECONDS` (default 300), `--infernet ORIGIN` (default `$W3BS_INFERNET_API` or https://infernetprotocol.com). A question that starts with `-` goes after `--`.

### Chat and `/ask`

`w3bs ask` with no question, on a terminal, opens a chat. Plain text asks the current agent; these slash commands do the rest:

| Command                   | Does                                 |
| ------------------------- | ------------------------------------ |
| `/ask <agent> <question>` | ask that agent (and make it current) |
| `/agent <name>`           | switch agent                         |
| `/list`                   | the agents                           |
| `/help`                   | the commands                         |
| `/quit`                   | leave (Ctrl-D works too)             |

```
business> /ask business "What do I do, I can't make money???? HELP!"
```

## The agents

| Agent      | Aliases              | Model                                                                                                                                                                                                                                                                   |
| ---------- | -------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `business` | `saas`, `strategist` | Profullstack's [saas-business-strategist-agent](https://huggingface.co/profullstack/saas-business-strategist-agent): Qwen2.5-0.5B-Instruct with a LoRA trained on the c0mpute network ([GGUF](https://huggingface.co/profullstack/saas-business-strategist-agent-GGUF)) |

The list lives at [w3bs.org/agents.json](/agents.json) and the CLI reads it on each run, so a new agent needs no new `w3bs`. A copy is built into the binary for when w3bs.org cannot be reached. Each agent is one object:

```json
{
  "name": "business",
  "aliases": ["saas", "strategist"],
  "title": "SaaS business strategist",
  "by": "Profullstack",
  "model": "hf.co/profullstack/saas-business-strategist-agent-GGUF:latest",
  "huggingface": "https://huggingface.co/profullstack/saas-business-strategist-agent",
  "gguf": "https://huggingface.co/profullstack/saas-business-strategist-agent-GGUF",
  "system": "You are saas-business-strategist-agent. ...",
  "grounding": { "search": "pit.world", "query_suffix": "startup revenue", "results": 3 },
  "max_tokens": 350,
  "temperature": 0.2
}
```

`model` is the name Infernet nodes serve it under. An agent is only useful when nodes serve its model: Infernet [house models](https://infernetprotocol.com/house-models.json) are pulled by every node that fits them, so publish the GGUF there first, then add the agent with a pull request to `public/agents.json` in [profullstack/w3bs.org](https://github.com/profullstack/w3bs.org).

## Grounding

Small models supply the voice, not the facts. Before asking, `w3bs ask` runs the question (plus the agent's `query_suffix`) through [pit.world](https://pit.world) search and puts the top results, numbered, in the prompt. Results with a description come first. They are printed as the sources. If the search fails, the agent answers without them and `w3bs` says so.

## Limits and keys

- Questions go to Infernet's public chat API (`POST /api/chat` with `p2p_only`, then the answer's event stream), which picks a live node serving the model. `p2p_only` means a question is refused rather than sent to a hosted model when no node is free; `w3bs` also refuses any answer that did not come from a node.
- Without a key, everyone at one IP address shares Infernet's public limit of **20 questions an hour**. Set `INFERNET_API_KEY` to an Infernet trusted client key for more; it is sent as a bearer token.
- An answer usually starts in 5 to 20 seconds on a CPU node. A node that has just pulled the model, or a busy one, takes longer.
- The question and the search results are sent to the node in plain text. Do not ask anything you would not post.
- `ASK_NO_NODE` means no live node serves the model right now; try again in a few minutes.

## Environment

| Variable            | Default                        |
| ------------------- | ------------------------------ |
| `INFERNET_API_KEY`  | none (anonymous, rate limited) |
| `W3BS_INFERNET_API` | `https://infernetprotocol.com` |
| `W3BS_AGENTS`       | `https://w3bs.org/agents.json` |
| `W3BS_SEARCH_URL`   | `https://pit.world/api/search` |

All must be HTTPS, except plain HTTP to loopback for development.
