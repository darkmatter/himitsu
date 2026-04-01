import {
  Box,
  Text,
  Input,
  Select,
  BoxRenderable,
  TextRenderable,
  InputRenderableEvents,
  SelectRenderableEvents,
  type CliRenderer,
  type KeyEvent,
  type SelectOption,
} from "@opentui/core";
import { colors } from "../theme";
import { clearChildren } from "../helpers";
import * as himitsu from "../himitsu";

export interface WizardDefaults {
  data_dir: string;
  state_dir: string;
  suggested_user: string;
  suggested_remote: string | null;
  keyring_available: boolean;
  already_initialized: boolean;
}

type WizardDone = (remote?: string) => void;

export function InitWizard(
  renderer: CliRenderer,
  onDone: WizardDone,
  defaults?: WizardDefaults,
) {
  let step = 0;
  let homeDir = defaults?.data_dir ?? "~/.local/share/himitsu";
  let remoteStore =
    defaults?.suggested_remote ??
    (defaults?.suggested_user ? `${defaults.suggested_user}/secrets` : "");
  let keyProvider = "disk";
  const keyringAvailable = defaults?.keyring_available ?? false;

  const body = new BoxRenderable(renderer, {
    id: "wizard-body",
    flexDirection: "column",
    flexGrow: 1,
    paddingLeft: 2,
  });

  // ── Esc handler (go back one step) ──────────────────────────────────
  const escHandler = (key: KeyEvent) => {
    if (key.name === "escape" && step > 0 && step < 3) {
      step--;
      renderStep();
    }
  };
  renderer.keyInput.on("keypress", escHandler);

  // ── Step rendering ──────────────────────────────────────────────────
  function renderStep() {
    clearChildren(body);
    switch (step) {
      case 0:
        renderHomeStep();
        break;
      case 1:
        renderRemoteStep();
        break;
      case 2:
        renderKeyringStep();
        break;
      case 3:
        renderConfirmStep();
        break;
    }
  }

  // ── Step 1: Himitsu Home ────────────────────────────────────────────
  function renderHomeStep() {
    body.add(
      new TextRenderable(renderer, {
        id: "s1-label",
        content: "Step 1 of 3 — Himitsu Home",
        fg: colors.yellow,
        marginBottom: 1,
      }),
    );
    body.add(
      new TextRenderable(renderer, {
        id: "s1-desc",
        content: "Where should himitsu store keys and config?",
        fg: colors.fgDim,
        marginBottom: 1,
      }),
    );

    const input = Input({
      value: homeDir,
      width: 60,
      backgroundColor: colors.bgDark,
      focusedBackgroundColor: colors.bgHighlight,
      textColor: colors.fg,
      cursorColor: colors.accent,
    });

    input.on(InputRenderableEvents.INPUT, (val: string) => {
      homeDir = val;
    });
    input.on(InputRenderableEvents.ENTER, () => {
      step = 1;
      renderStep();
    });

    body.add(input);
    input.focus();

    body.add(
      new TextRenderable(renderer, {
        id: "s1-hint",
        content: "Enter to continue",
        fg: colors.fgMuted,
        marginTop: 1,
      }),
    );
  }

  // ── Step 2: Remote Store ────────────────────────────────────────────
  function renderRemoteStep() {
    body.add(
      new TextRenderable(renderer, {
        id: "s2-label",
        content: "Step 2 of 3 — Remote Store",
        fg: colors.yellow,
        marginBottom: 1,
      }),
    );
    body.add(
      new TextRenderable(renderer, {
        id: "s2-desc",
        content: "Default secret store (org/repo on GitHub):",
        fg: colors.fgDim,
        marginBottom: 1,
      }),
    );

    const input = Input({
      value: remoteStore,
      placeholder: "org/repo",
      width: 60,
      backgroundColor: colors.bgDark,
      focusedBackgroundColor: colors.bgHighlight,
      textColor: colors.fg,
      cursorColor: colors.accent,
    });

    input.on(InputRenderableEvents.INPUT, (val: string) => {
      remoteStore = val;
    });
    input.on(InputRenderableEvents.ENTER, () => {
      step = 2;
      renderStep();
    });

    body.add(input);
    input.focus();

    body.add(
      new TextRenderable(renderer, {
        id: "s2-hint",
        content: "Enter to continue · Esc to go back",
        fg: colors.fgMuted,
        marginTop: 1,
      }),
    );
  }

  // ── Step 3: Keyring ─────────────────────────────────────────────────
  function renderKeyringStep() {
    body.add(
      new TextRenderable(renderer, {
        id: "s3-label",
        content: "Step 3 of 3 — Key Provider",
        fg: colors.yellow,
        marginBottom: 1,
      }),
    );

    if (!keyringAvailable) {
      body.add(
        new TextRenderable(renderer, {
          id: "s3-unavail",
          content: "OS keyring is not available on this platform.",
          fg: colors.fgDim,
          marginBottom: 1,
        }),
      );
      body.add(
        new TextRenderable(renderer, {
          id: "s3-skip",
          content: "Keys will be stored on disk. Press any key to continue.",
          fg: colors.fgMuted,
        }),
      );
      const skipHandler = () => {
        renderer.keyInput.removeListener("keypress", skipHandler);
        keyProvider = "disk";
        step = 3;
        renderStep();
      };
      renderer.keyInput.on("keypress", skipHandler);
      return;
    }

    body.add(
      new TextRenderable(renderer, {
        id: "s3-desc",
        content: "Store age keys in the OS keychain?",
        fg: colors.fgDim,
        marginBottom: 1,
      }),
    );

    const options: SelectOption[] = [
      {
        name: "Yes",
        description: "Use macOS Keychain for key storage",
        value: "macos-keychain",
      },
      { name: "No", description: "Keep keys on disk only", value: "disk" },
    ];

    const select = Select({
      width: 50,
      height: 4,
      options,
      backgroundColor: colors.bgDark,
      selectedBackgroundColor: colors.bgSelected,
      selectedTextColor: colors.accent,
      textColor: colors.fg,
      showDescription: true,
      wrapSelection: true,
    });

    select.on(
      SelectRenderableEvents.ITEM_SELECTED,
      (_index: number, option: SelectOption) => {
        keyProvider = option.value ?? "disk";
        step = 3;
        renderStep();
      },
    );

    body.add(select);
    select.focus();

    body.add(
      new TextRenderable(renderer, {
        id: "s3-hint",
        content: "Enter to select · Esc to go back",
        fg: colors.fgMuted,
        marginTop: 1,
      }),
    );
  }

  // ── Confirmation & init ─────────────────────────────────────────────
  function renderConfirmStep() {
    body.add(
      new TextRenderable(renderer, {
        id: "c-label",
        content: "Setting up himitsu…",
        fg: colors.yellow,
        marginBottom: 1,
      }),
    );

    try {
      const isDefaultHome = homeDir === (defaults?.data_dir ?? "");
      const result = himitsu.initWithOptions({
        name: remoteStore || undefined,
        home: isDefaultHome ? undefined : homeDir,
        keyProvider: keyProvider !== "disk" ? keyProvider : undefined,
      });

      // ── Show results ──────────────────────────────────────────────
      if (!result.key_existed) {
        body.add(
          new TextRenderable(renderer, {
            id: "c-key",
            content: "✓ Created age keypair",
            fg: colors.green,
          }),
        );
        body.add(
          new TextRenderable(renderer, {
            id: "c-pubkey",
            content: `  Public key: ${result.pubkey}`,
            fg: colors.fg,
            marginBottom: 1,
          }),
        );
      } else {
        body.add(
          new TextRenderable(renderer, {
            id: "c-key-exists",
            content: "✓ Age keypair ready",
            fg: colors.green,
          }),
        );
        body.add(
          new TextRenderable(renderer, {
            id: "c-pubkey",
            content: `  Public key: ${result.pubkey}`,
            fg: colors.fgDim,
            marginBottom: 1,
          }),
        );
      }

      body.add(
        new TextRenderable(renderer, {
          id: "c-home",
          content: `✓ Home: ${result.data_dir}`,
          fg: colors.green,
        }),
      );

      if (remoteStore) {
        body.add(
          new TextRenderable(renderer, {
            id: "c-store",
            content: `✓ Store: ${remoteStore} (default)`,
            fg: colors.green,
          }),
        );
      }

      body.add(
        new TextRenderable(renderer, {
          id: "c-provider",
          content: `✓ Key provider: ${keyProvider}`,
          fg: colors.green,
        }),
      );

      body.add(
        new TextRenderable(renderer, {
          id: "c-done",
          content: "Press any key to continue.",
          fg: colors.fgDim,
          marginTop: 2,
        }),
      );

      renderer.keyInput.once("keypress", () => {
        renderer.keyInput.removeListener("keypress", escHandler);
        onDone(remoteStore || undefined);
      });
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      body.add(
        new TextRenderable(renderer, {
          id: "c-error",
          content: `Error: ${msg}`,
          fg: colors.red,
          marginTop: 1,
        }),
      );
      body.add(
        new TextRenderable(renderer, {
          id: "c-retry",
          content: "Press Esc to go back and try again.",
          fg: colors.fgDim,
          marginTop: 1,
        }),
      );
    }
  }

  // ── Initial render ──────────────────────────────────────────────────
  renderStep();

  return Box(
    { flexDirection: "column", flexGrow: 1 },
    Box(
      { flexDirection: "column", marginBottom: 1 },
      Text({ content: " ✦ himitsu setup", fg: colors.accent }),
    ),
    body,
  );
}
