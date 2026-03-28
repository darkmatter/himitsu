import { createCliRenderer } from "@opentui/core"
import { createApp } from "./app"

// Optional: override with `bun run dev myorg/secrets`
const remote = process.argv[2]

const renderer = await createCliRenderer({
  exitOnCtrlC: true,
  targetFps: 30,
})

const app = createApp(renderer, remote)
renderer.root.add(app)
