import {
  Box, Text,
  BoxRenderable,
  TextRenderable,
  type CliRenderer,
} from "@opentui/core"
import { colors } from "../theme"
import { clearChildren } from "../helpers"
import * as himitsu from "../himitsu"

type WizardDone = (store?: string) => void

export function InitWizard(renderer: CliRenderer, onDone: WizardDone) {
  const body = new BoxRenderable(renderer, {
    id: "wizard-body",
    flexDirection: "column",
    flexGrow: 1,
    paddingLeft: 2,
  })

  // Run init and show the result
  const result = himitsu.init()

  if (result.store_existed && result.home_existed) {
    // Already initialized -- show status and continue
    body.add(new TextRenderable(renderer, {
      id: "w-store",
      content: `Store: ${result.store}`,
      fg: colors.fg,
    }))
    body.add(new TextRenderable(renderer, {
      id: "w-key",
      content: `Key:   ${result.pubkey}`,
      fg: colors.fgDim,
      marginTop: 1,
    }))
    if (result.in_git_repo) {
      body.add(new TextRenderable(renderer, {
        id: "w-git",
        content: "Project-local store (committed with repo)",
        fg: colors.green,
        marginTop: 1,
      }))
    }
    body.add(new TextRenderable(renderer, {
      id: "w-continue",
      content: "Press any key to continue.",
      fg: colors.fgDim,
      marginTop: 2,
    }))
    renderer.keyInput.once("keypress", () => {
      onDone(result.store)
    })
  } else {
    // Fresh init
    if (!result.home_existed) {
      body.add(new TextRenderable(renderer, {
        id: "w-keygen",
        content: `Created keyring at ${result.user_home}`,
        fg: colors.green,
      }))
      body.add(new TextRenderable(renderer, {
        id: "w-key",
        content: `Age key: ${result.pubkey}`,
        fg: colors.fg,
        marginTop: 1,
      }))
    }

    body.add(new TextRenderable(renderer, {
      id: "w-store-created",
      content: `Initialized store at ${result.store}`,
      fg: colors.green,
      marginTop: 1,
    }))

    body.add(new TextRenderable(renderer, {
      id: "w-recipient",
      content: "Added self as recipient (common/self)",
      fg: colors.fg,
      marginTop: 1,
    }))

    if (result.in_git_repo) {
      body.add(new TextRenderable(renderer, {
        id: "w-git-hint",
        content: "Secrets will be stored in .himitsu/ alongside your code.",
        fg: colors.fgDim,
        marginTop: 1,
      }))
      if (result.suggested_remote) {
        body.add(new TextRenderable(renderer, {
          id: "w-origin",
          content: `Git origin: ${result.suggested_remote}`,
          fg: colors.fgDim,
          marginTop: 1,
        }))
      }
    } else {
      body.add(new TextRenderable(renderer, {
        id: "w-global-hint",
        content: "Using global store (not inside a git repo).",
        fg: colors.fgDim,
        marginTop: 1,
      }))
    }

    body.add(new TextRenderable(renderer, {
      id: "w-ready",
      content: "Ready. Press any key to continue.",
      fg: colors.fgDim,
      marginTop: 2,
    }))

    renderer.keyInput.once("keypress", () => {
      onDone(result.store)
    })
  }

  return Box(
    { flexDirection: "column", flexGrow: 1 },
    Box(
      { flexDirection: "column", marginBottom: 1 },
      Text({ content: " himitsu init", fg: colors.accent }),
    ),
    body,
  )
}
