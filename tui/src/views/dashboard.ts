import {
  Box, Text, Select,
  BoxRenderable,
  type CliRenderer,
  type SelectOption,
  SelectRenderableEvents,
} from "@opentui/core"
import { colors } from "../theme"
import { clearChildren } from "../helpers"
import * as himitsu from "../himitsu"

export function Dashboard(renderer: CliRenderer, remote: string | undefined, onAction: (action: string, data?: any) => void) {
  const envs = himitsu.listEnvs(remote)
  const label = remote ?? "local"

  const envOptions: SelectOption[] = envs.map((e) => ({
    name: e,
    description: "",
  }))

  if (envOptions.length === 0) {
    return Box(
      { flexDirection: "column", padding: 1, flexGrow: 1 },
      Text({ content: `Remote: ${label}`, fg: colors.accent }),
      Text({ content: "" }),
      Text({ content: "No environments found.", fg: colors.fgDim }),
      Text({ content: "Use `himitsu set <env> <key> <value>` to create secrets.", fg: colors.fgDim }),
    )
  }

  let currentEnv = envs[0]

  const secretPanel = new BoxRenderable(renderer, {
    id: "secret-panel",
    flexDirection: "column",
    flexGrow: 1,
    marginLeft: 2,
  })

  function refreshSecrets(env: string) {
    currentEnv = env
    const secrets = himitsu.listSecrets(env, remote)

    clearChildren(secretPanel)

    const items = [
      new (Text as any)({ content: `  ${env}`, fg: colors.yellow, marginBottom: 1 }),
      ...secrets.map((key: string) =>
        new (Text as any)({ content: `  ${key}`, fg: colors.fg })
      ),
    ]

    if (secrets.length === 0) {
      items.push(new (Text as any)({ content: "  (empty)", fg: colors.fgDim }))
    }

    items.push(
      new (Text as any)({ content: "" }),
      new (Text as any)({ content: "  enter  reveal    /  search    e  re-encrypt", fg: colors.fgDim }),
    )

    for (const item of items) {
      secretPanel.add(item)
    }
  }

  const envSelect = Select({
    width: 24,
    height: Math.min(envOptions.length + 2, 20),
    options: envOptions,
    backgroundColor: colors.bgDark,
    selectedBackgroundColor: colors.bgSelected,
    selectedTextColor: colors.accent,
    textColor: colors.fg,
    showDescription: false,
    wrapSelection: true,
  })

  envSelect.on(SelectRenderableEvents.SELECTION_CHANGED, (_index: number, option: SelectOption) => {
    refreshSecrets(option.name)
  })

  envSelect.on(SelectRenderableEvents.ITEM_SELECTED, (_index: number, option: SelectOption) => {
    onAction("select-env", option.name)
  })

  envSelect.focus()
  refreshSecrets(currentEnv)

  return Box(
    { flexDirection: "column", flexGrow: 1 },
    Box(
      { flexDirection: "row", marginBottom: 1 },
      Text({ content: ` ${label}`, fg: colors.accent }),
      Text({ content: `  ${envs.length} env(s)`, fg: colors.fgDim }),
    ),
    Box(
      { flexDirection: "row", flexGrow: 1 },
      Box(
        { flexDirection: "column", width: 26 },
        Text({ content: " Environments", fg: colors.fgDim, marginBottom: 1 }),
        envSelect,
      ),
      Box(
        { flexDirection: "column", flexGrow: 1 },
        Text({ content: " Secrets", fg: colors.fgDim, marginBottom: 1 }),
        secretPanel,
      ),
    ),
  )
}
