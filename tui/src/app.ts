import {
  Box, Text,
  BoxRenderable,
  type CliRenderer,
  type KeyEvent,
} from "@opentui/core"
import { colors } from "./theme"
import { clearChildren } from "./helpers"
import { Dashboard } from "./views/dashboard"
import { SearchView } from "./views/search"
import { SecretView } from "./views/secret"
import { InitWizard } from "./views/init-wizard"
import * as himitsu from "./himitsu"

type View = "wizard" | "dashboard" | "search" | "secret"

interface AppState {
  view: View
  remote?: string
  selectedEnv?: string
}

export function createApp(renderer: CliRenderer, remote?: string) {
  // Check if himitsu is initialized
  const needsInit = !remote && !isInitialized()

  const state: AppState = {
    view: needsInit ? "wizard" : "dashboard",
    remote,
  }

  const content = new BoxRenderable(renderer, {
    id: "content",
    flexDirection: "column",
    flexGrow: 1,
  })

  function navigate(view: View, data?: any) {
    state.view = view
    if (data?.env) state.selectedEnv = data.env
    if (data?.remote) state.remote = data.remote
    renderView()
  }

  function handleAction(action: string, data?: any) {
    switch (action) {
      case "select-env":
        navigate("secret", { env: data })
        break
      case "back":
        navigate("dashboard")
        break
    }
  }

  function renderView() {
    clearChildren(content)

    switch (state.view) {
      case "wizard":
        content.add(InitWizard(renderer, (remote) => {
          state.remote = remote
          navigate("dashboard")
        }))
        break
      case "dashboard":
        content.add(Dashboard(renderer, state.remote, handleAction))
        break
      case "search":
        content.add(SearchView(renderer, handleAction))
        break
      case "secret":
        content.add(SecretView(renderer, state.remote, state.selectedEnv!, handleAction))
        break
    }
  }

  renderer.keyInput.on("keypress", (key: KeyEvent) => {
    // Don't intercept keys during wizard
    if (state.view === "wizard") return

    if (key.name === "escape") {
      if (state.view !== "dashboard") {
        navigate("dashboard")
      }
      return
    }

    if (key.name === "/" && state.view === "dashboard") {
      navigate("search")
      return
    }

    if (key.name === "q" && !key.ctrl && state.view === "dashboard") {
      renderer.destroy()
      return
    }
  })

  const header = Box(
    {
      flexDirection: "row",
      height: 1,
      marginBottom: 1,
    },
    Text({ content: " himitsu ", fg: colors.bg, bg: colors.accent }),
    Text({ content: "  " }),
    Text({ content: "q quit  / search  esc back", fg: colors.fgDim }),
  )

  const root = Box(
    {
      flexDirection: "column",
      width: "100%",
      height: "100%",
      backgroundColor: colors.bg,
      padding: 1,
    },
    header,
    content,
  )

  renderView()
  return root
}

/** Check if himitsu has been initialized (keys exist). */
function isInitialized(): boolean {
  try {
    const result = himitsu.init()
    return result.store_existed
  } catch {
    return false
  }
}
