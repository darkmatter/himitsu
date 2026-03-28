# nix/lib/default.nix
#
# Himitsu Nix library — devshell integration, secret packaging,
# credential helpers, container primitives, and convenience wrappers.
#
# Imported from flake.nix:
#   lib = import ./nix/lib { inherit pkgs himitsu age-key-cmd; };
#
# ── Quick start ───────────────────────────────────────────────────────
#
#   devShells.default = himitsu.lib.${system}.mkDevShell {
#     devShell = pkgs.mkShell { packages = [ nodejs ]; };
#     store    = ./.himitsu;
#     env      = "dev";
#   };
#
# ── Full API surface ─────────────────────────────────────────────────
#
#   packSecrets              — collect .age files into a derivation
#   mkDevShell               — wrap any devShell with secret injection
#   wrapAge                  — age binary that auto-injects identity
#   wrapSops                 — sops binary that auto-discovers key
#   mkCredentialServer       — standalone HTTP credential server script
#   mkEntrypoint             — container ENTRYPOINT that decrypts + execs
#   mkSecretsLayer           — tar / derivation of encrypted secrets for images
#   mkCredentialServerImage  — minimal OCI image with the credential server

{
  pkgs,
  himitsu,
  age-key-cmd,
}:

let
  inherit (pkgs) lib;
  age = pkgs.age;
  jq = pkgs.jq;

  # ===================================================================
  #  Internal: credential server (stdlib-only Python)
  # ===================================================================

  credentialServerPy = pkgs.writeText "himitsu-credential-server.py" ''
    """
    Himitsu credential server — tiny HTTP service serving decrypted secrets.

    Designed to be plugged into:
      * AWS SDK  (via AWS_CONTAINER_CREDENTIALS_FULL_URI)
      * SOPS     (via SOPS_AGE_KEY_FILE or /v1/sops-age-key)
      * Any HTTP consumer

    Security: 127.0.0.1 by default, Bearer token required on every request.
    """
    import argparse, base64, http.server, json, os, secrets as _secrets, signal, sys

    _cache = {}
    _age_identity = None
    _auth_token = None

    class Handler(http.server.BaseHTTPRequestHandler):
        server_version = "himitsu/1.0"
        protocol_version = "HTTP/1.1"

        def log_message(self, fmt, *args):
            if os.environ.get("HIMITSU_SERVER_QUIET") != "1":
                super().log_message(fmt, *args)

        def _check_auth(self):
            if _auth_token is None:
                return True
            hdr = self.headers.get("Authorization", "")
            if hdr == f"Bearer {_auth_token}":
                return True
            self._json(403, {"error": "forbidden"})
            return False

        def _json(self, code, body):
            payload = json.dumps(body).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def _text(self, code, body):
            payload = body.encode() if isinstance(body, str) else body
            self.send_response(code)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def do_GET(self):
            if not self._check_auth():
                return
            p = self.path.rstrip("/")

            if p == "/v1/health":
                self._json(200, {"status": "ok", "secrets": len(_cache)})

            elif p == "/v1/secrets":
                self._json(200, _cache)

            elif p.startswith("/v1/secrets/"):
                key = p[len("/v1/secrets/"):]
                if key in _cache:
                    self._text(200, _cache[key])
                else:
                    self._json(404, {"error": f"not found: {key}"})

            elif p == "/v1/secrets-b64":
                blob = base64.b64encode(json.dumps(_cache).encode()).decode()
                self._text(200, blob)

            elif p == "/v1/env":
                lines = [f"{k}={v}" for k, v in sorted(_cache.items())]
                self._text(200, "\n".join(lines) + "\n")

            elif p == "/credentials":
                ak = _cache.get("AWS_ACCESS_KEY_ID", "")
                if not ak:
                    self._json(404, {"error": "AWS_ACCESS_KEY_ID not in secrets"})
                else:
                    self._json(200, {
                        "AccessKeyId": ak,
                        "SecretAccessKey": _cache.get("AWS_SECRET_ACCESS_KEY", ""),
                        "Token": _cache.get("AWS_SESSION_TOKEN", ""),
                        "Expiration": "2099-12-31T23:59:59Z",
                    })

            elif p in ("/v1/age-identity", "/v1/sops-age-key"):
                if _age_identity:
                    self._text(200, _age_identity)
                else:
                    self._json(404, {"error": "age identity not available"})

            else:
                self._json(404, {"error": "not found"})

    def load_secrets(d):
        if not os.path.isdir(d):
            return
        for name in sorted(os.listdir(d)):
            fp = os.path.join(d, name)
            if os.path.isfile(fp):
                with open(fp) as f:
                    _cache[name] = f.read()
        print(f"loaded {len(_cache)} secrets", file=sys.stderr)

    def main():
        global _auth_token, _age_identity
        ap = argparse.ArgumentParser()
        ap.add_argument("--secrets-dir", required=True)
        ap.add_argument("--port", type=int, default=9292)
        ap.add_argument("--host", default="127.0.0.1")
        ap.add_argument("--identity-file", default=None)
        ap.add_argument("--token", default=None)
        ap.add_argument("--token-file", default=None)
        args = ap.parse_args()

        _auth_token = args.token or os.environ.get("HIMITSU_SERVER_TOKEN") or _secrets.token_urlsafe(32)

        load_secrets(args.secrets_dir)

        if args.identity_file and os.path.isfile(args.identity_file):
            with open(args.identity_file) as f:
                _age_identity = "\n".join(
                    l.strip() for l in f if l.strip() and not l.startswith("#")
                )

        if args.token_file:
            with open(args.token_file, "w") as f:
                f.write(_auth_token)
            os.chmod(args.token_file, 0o600)

        print(f"HIMITSU_SERVER_TOKEN={_auth_token}", file=sys.stderr)
        print(f"HIMITSU_SERVER_URL=http://{args.host}:{args.port}", file=sys.stderr)

        srv = http.server.HTTPServer((args.host, args.port), Handler)
        signal.signal(signal.SIGTERM, lambda *_: (srv.shutdown(), sys.exit(0)))
        signal.signal(signal.SIGINT, lambda *_: (srv.shutdown(), sys.exit(0)))
        print(f"listening on {args.host}:{args.port}", file=sys.stderr)
        srv.serve_forever()

    if __name__ == "__main__":
        main()
  '';

  # ===================================================================
  #  Internal: shellHook that decrypts secrets at `nix develop` time
  # ===================================================================

  mkDecryptHook =
    {
      secretsPkg,
      exportEnv ? true,
      jsonEnv ? true,
      plaintextDir ? null,
      useRamfs ? true,
      credServer ? {
        enable = false;
        port = 9292;
        awsProfile = null;
        sops = false;
      },
      identityFile ? null,
      verbose ? false,
    }:
    let
      serverPort = toString (credServer.port or 9292);
      enableCredSrv = credServer.enable or false;
      enableAws = credServer.awsProfile or null;
    in
    ''
      # ── Himitsu: resolve age identity ──────────────────────────────
      _himitsu_identity="''${HIMITSU_IDENTITY:-${
        if identityFile != null then identityFile else "\"$HOME/.himitsu/keys/age.txt\""
      }}"

      if [ ! -f "$_himitsu_identity" ]; then
        echo "⚠  himitsu: age identity not found at $_himitsu_identity" >&2
        echo "   Run 'himitsu init' first, or set HIMITSU_IDENTITY." >&2
      else

      # ── Himitsu: create secure tmpdir ──────────────────────────────
      ${
        if useRamfs then
          ''
            if [ -d /dev/shm ]; then
              _himitsu_tmpdir="$(mktemp -d /dev/shm/himitsu.XXXXXX)"
            else
              _himitsu_tmpdir="$(mktemp -d "''${TMPDIR:-/tmp}/himitsu.XXXXXX")"
            fi
          ''
        else
          ''
            _himitsu_tmpdir="$(mktemp -d "''${TMPDIR:-/tmp}/himitsu.XXXXXX")"
          ''
      }
      chmod 700 "$_himitsu_tmpdir"

      _himitsu_plaintext_dir="${
        if plaintextDir != null then plaintextDir else "\"$_himitsu_tmpdir/plaintext\""
      }"
      mkdir -p "$_himitsu_plaintext_dir"
      chmod 700 "$_himitsu_plaintext_dir"

      # ── Himitsu: decrypt ───────────────────────────────────────────
      _himitsu_count=0
      _himitsu_failed=0
      for _f in ${secretsPkg}/secrets/*.age; do
        [ -f "$_f" ] || continue
        _name="$(basename "$_f" .age)"
        if ${age}/bin/age --decrypt -i "$_himitsu_identity" \
             -o "$_himitsu_plaintext_dir/$_name" "$_f" 2>/dev/null; then
          chmod 600 "$_himitsu_plaintext_dir/$_name"
          _himitsu_count=$(( _himitsu_count + 1 ))
          ${lib.optionalString verbose ''echo "  ✓ $_name" >&2''}
        else
          _himitsu_failed=$(( _himitsu_failed + 1 ))
          echo "  ✗ $_name (decrypt failed)" >&2
        fi
      done

      export HIMITSU_SECRETS_DIR="$_himitsu_plaintext_dir"
      export HIMITSU_IDENTITY="$_himitsu_identity"
      export HIMITSU_SECRETS_COUNT="$_himitsu_count"

      ${lib.optionalString exportEnv ''
        # ── export individual env vars ───────────────────────────────
        for _f in "$_himitsu_plaintext_dir"/*; do
          [ -f "$_f" ] || continue
          _name="$(basename "$_f")"
          IFS= read -r -d "" _val < "$_f" || true
          export "$_name=$_val"
        done
      ''}

      ${lib.optionalString jsonEnv ''
        # ── build JSON blob ──────────────────────────────────────────
        _himitsu_json="{}"
        for _f in "$_himitsu_plaintext_dir"/*; do
          [ -f "$_f" ] || continue
          _name="$(basename "$_f")"
          _himitsu_json="$(echo "$_himitsu_json" | \
            ${jq}/bin/jq --arg k "$_name" --rawfile v "$_f" '. + {($k): $v}')"
        done
        export HIMITSU_SECRETS_JSON="$(echo "$_himitsu_json" | base64)"
        export HIMITSU_SECRETS_JSON_RAW="$_himitsu_json"
      ''}

      # ── SOPS compat ───────────────────────────────────────────────
      export SOPS_AGE_KEY_FILE="$_himitsu_identity"

      ${lib.optionalString enableCredSrv ''
        # ── credential server ────────────────────────────────────────
        _himitsu_token_file="$_himitsu_tmpdir/.server-token"
        ${pkgs.python3}/bin/python3 ${credentialServerPy} \
          --secrets-dir "$_himitsu_plaintext_dir" \
          --port ${serverPort} \
          --identity-file "$_himitsu_identity" \
          --token-file "$_himitsu_token_file" \
          2>"$_himitsu_tmpdir/.server-log" &
        _himitsu_server_pid=$!
        sleep 0.3
        if [ -f "$_himitsu_token_file" ]; then
          export HIMITSU_SERVER_TOKEN="$(cat "$_himitsu_token_file")"
          export HIMITSU_SERVER_URL="http://127.0.0.1:${serverPort}"
          export HIMITSU_SERVER_PID="$_himitsu_server_pid"
          ${lib.optionalString (enableAws != null) ''
            export AWS_CONTAINER_CREDENTIALS_FULL_URI="http://127.0.0.1:${serverPort}/credentials"
            export AWS_CONTAINER_AUTHORIZATION_TOKEN="Bearer $HIMITSU_SERVER_TOKEN"
          ''}
        else
          echo "⚠  himitsu: credential server failed to start" >&2
        fi
      ''}

      if [ "$_himitsu_count" -gt 0 ]; then
        echo "🔓 himitsu: decrypted $_himitsu_count secret(s) into dev shell" >&2
        ${lib.optionalString enableCredSrv ''
          echo "   credential server → http://127.0.0.1:${serverPort}" >&2
        ''}
      fi
      if [ "$_himitsu_failed" -gt 0 ]; then
        echo "⚠  himitsu: $_himitsu_failed secret(s) failed to decrypt" >&2
      fi

      # ── cleanup on exit ────────────────────────────────────────────
      _himitsu_cleanup() {
        ${lib.optionalString enableCredSrv ''
          [ -n "''${_himitsu_server_pid:-}" ] && kill "$_himitsu_server_pid" 2>/dev/null || true
        ''}
        [ -n "''${_himitsu_tmpdir:-}" ] && [ -d "$_himitsu_tmpdir" ] && rm -rf "$_himitsu_tmpdir"
        ${lib.optionalString (plaintextDir != null) ''
          [ -d "${plaintextDir}" ] && rm -rf "${plaintextDir}"
        ''}
      }
      trap _himitsu_cleanup EXIT

      fi  # end identity-file check
    '';

  # ===================================================================
  #  Internal: resolve a secrets derivation from user-facing args
  # ===================================================================

  resolveSecrets =
    {
      secrets ? null,
      store ? null,
      env ? "dev",
      mergeEnvs ? [ "common" ],
    }:
    if secrets != null then
      if builtins.isList secrets then
        pkgs.symlinkJoin {
          name = "himitsu-secrets-merged";
          paths = secrets;
        }
      else
        secrets
    else if store != null then
      packSecrets {
        src = store;
        inherit env mergeEnvs;
      }
    else
      builtins.throw "himitsu: provide either 'secrets' or 'store'";

  # ===================================================================
  #  packSecrets
  # ===================================================================
  #
  #  Collect encrypted .age files into a clean derivation.
  #
  #  Two calling conventions:
  #
  #    packSecrets ./.himitsu/vars/dev
  #
  #    packSecrets {
  #      src       = ./.himitsu;
  #      env       = "dev";
  #      mergeEnvs = [ "common" ];
  #    }
  #
  #  Output:
  #    $out/secrets/*.age
  #    $out/manifest.txt
  #

  packSecrets =
    srcOrAttrs:
    let
      attrs = if builtins.isAttrs srcOrAttrs then srcOrAttrs else { src = srcOrAttrs; };

      src = attrs.src;
      env = attrs.env or "dev";
      mergeEnvs = attrs.mergeEnvs or [ "common" ];
      name = attrs.name or "himitsu-secrets-${env}";

      # Detect: is `src` a single env dir (contains .age files directly)
      # or a full store (contains vars/)?
      isEnvDir = !(builtins.pathExists (src + "/vars"));
    in
    pkgs.runCommand name
      {
        inherit src;
        preferLocalBuild = true;
      }
      (
        if isEnvDir then
          ''
            mkdir -p $out/secrets
            for f in $src/*.age; do
              [ -f "$f" ] && cp "$f" $out/secrets/ || true
            done
            ls -1 $out/secrets/ 2>/dev/null | sed 's/\.age$//' | sort -u > $out/manifest.txt
          ''
        else
          ''
            mkdir -p $out/secrets

            # Lower-priority environments first (cp -n = no clobber).
            ${lib.concatMapStringsSep "\n" (e: ''
              if [ -d "$src/vars/${e}" ]; then
                for f in $src/vars/${e}/*.age; do
                  [ -f "$f" ] && cp -n "$f" $out/secrets/ 2>/dev/null || true
                done
              fi
            '') mergeEnvs}

            # Target environment last (overwrites).
            if [ -d "$src/vars/${env}" ]; then
              cp $src/vars/${env}/*.age $out/secrets/ 2>/dev/null || true
            fi

            ls -1 $out/secrets/ 2>/dev/null | sed 's/\.age$//' | sort -u > $out/manifest.txt
          ''
      );

  # ===================================================================
  #  wrapAge
  # ===================================================================
  #
  #  Wrapped `age` that auto-injects -i on decrypt when no identity
  #  is already specified.  Encrypt is passed through untouched.
  #

  wrapAge =
    {
      identityFile ? null,
    }:
    pkgs.writeShellScriptBin "age" ''
      _id="''${HIMITSU_IDENTITY:-${
        if identityFile != null then identityFile else "\"\$HOME/.himitsu/keys/age.txt\""
      }}"
      _needs_id=false _has_id=false
      for _a in "$@"; do
        case "$_a" in
          --decrypt|-d) _needs_id=true ;;
          --identity|-i) _has_id=true ;;
        esac
      done
      if $_needs_id && ! $_has_id && [ -f "$_id" ]; then
        exec ${age}/bin/age -i "$_id" "$@"
      else
        exec ${age}/bin/age "$@"
      fi
    '';

  # ===================================================================
  #  wrapSops
  # ===================================================================
  #
  #  Wrapped `sops` that sets SOPS_AGE_KEY_FILE automatically.
  #

  wrapSops =
    {
      identityFile ? null,
    }:
    let
      sops =
        pkgs.sops
          or (builtins.throw "himitsu.wrapSops: pkgs.sops is not available — add sops to your nixpkgs overlay");
    in
    pkgs.writeShellScriptBin "sops" ''
      export SOPS_AGE_KEY_FILE="''${HIMITSU_IDENTITY:-${
        if identityFile != null then identityFile else "\"\$HOME/.himitsu/keys/age.txt\""
      }}"
      exec ${sops}/bin/sops "$@"
    '';

  # ===================================================================
  #  mkDevShell
  # ===================================================================
  #
  #  Wrap any devShell with automatic secret injection.
  #
  #    mkDevShell {
  #      devShell = pkgs.mkShell { packages = [ nodejs ]; };
  #      store    = ./.himitsu;
  #      env      = "dev";
  #    }
  #

  mkDevShell =
    {
      devShell ? null,
      secrets ? null,
      store ? null,
      env ? "dev",
      mergeEnvs ? [ "common" ],
      exportEnv ? true,
      jsonEnv ? true,
      plaintext ? { },
      credentialServer ? { },
      identityFile ? null,
      extraPackages ? [ ],
      verbose ? false,
    }:
    let
      plaintextCfg = {
        enable = false;
        dir = null;
      }
      // plaintext;
      credServerCfg = {
        enable = false;
        port = 9292;
        awsProfile = null;
        sops = false;
      }
      // credentialServer;

      resolved = resolveSecrets {
        inherit
          secrets
          store
          env
          mergeEnvs
          ;
      };

      hook = mkDecryptHook {
        secretsPkg = resolved;
        inherit
          exportEnv
          jsonEnv
          verbose
          identityFile
          ;
        plaintextDir = plaintextCfg.dir;
        credServer = credServerCfg;
      };

      innerHook = if devShell != null && devShell ? shellHook then devShell.shellHook else "";

      wrappedAge = wrapAge { inherit identityFile; };
    in
    pkgs.mkShell {
      inputsFrom = lib.optional (devShell != null) devShell;

      packages = [
        himitsu
        wrappedAge
        jq
      ]
      ++ lib.optional credServerCfg.enable pkgs.python3
      ++ extraPackages;

      shellHook = ''
        ${innerHook}
        ${hook}
      '';

      HIMITSU_ENV = env;
      HIMITSU_STORE = if store != null then toString store else "";
    };

  # ===================================================================
  #  mkCredentialServer
  # ===================================================================
  #
  #  Standalone script: decrypt + serve over HTTP.
  #
  #    mkCredentialServer {
  #      store = ./.himitsu;
  #      env   = "prod";
  #      port  = 9292;
  #    }
  #

  mkCredentialServer =
    {
      secrets ? null,
      store ? null,
      env ? "dev",
      mergeEnvs ? [ "common" ],
      port ? 9292,
      host ? "127.0.0.1",
    }:
    let
      resolved = resolveSecrets {
        inherit
          secrets
          store
          env
          mergeEnvs
          ;
      };
    in
    pkgs.writeShellScriptBin "himitsu-credential-server" ''
      set -euo pipefail

      IDENTITY="''${HIMITSU_IDENTITY:-''${1:-$HOME/.himitsu/keys/age.txt}}"
      PORT="${toString port}"
      HOST="${host}"

      while [ $# -gt 0 ]; do
        case "$1" in
          --identity|-i) IDENTITY="$2"; shift 2 ;;
          --port|-p)     PORT="$2";     shift 2 ;;
          --host|-H)     HOST="$2";     shift 2 ;;
          *)             shift ;;
        esac
      done

      if [ ! -f "$IDENTITY" ]; then
        echo "error: age identity not found at $IDENTITY" >&2
        echo "usage: himitsu-credential-server [--identity FILE] [--port PORT]" >&2
        exit 1
      fi

      WORKDIR="$(mktemp -d)"
      trap 'rm -rf "$WORKDIR"' EXIT
      chmod 700 "$WORKDIR"
      PLAIN="$WORKDIR/secrets"
      mkdir -p "$PLAIN"

      for f in ${resolved}/secrets/*.age; do
        [ -f "$f" ] || continue
        name="$(basename "$f" .age)"
        ${age}/bin/age --decrypt -i "$IDENTITY" -o "$PLAIN/$name" "$f" 2>/dev/null \
          && chmod 600 "$PLAIN/$name" \
          || echo "  warning: $name failed" >&2
      done

      exec ${pkgs.python3}/bin/python3 ${credentialServerPy} \
        --secrets-dir "$PLAIN" \
        --port "$PORT" \
        --host "$HOST" \
        --identity-file "$IDENTITY"
    '';

  # ===================================================================
  #  mkEntrypoint
  # ===================================================================
  #
  #  Container ENTRYPOINT script that decrypts .age files, optionally
  #  exports env vars, optionally starts the credential server, then
  #  exec's into CMD.
  #
  #  All behaviour is controllable via env vars at container runtime,
  #  so you can rebuild the image once and change policy at deploy time.
  #
  #  Env vars consumed at runtime:
  #    HIMITSU_IDENTITY          — path to age identity (default /identity/age.txt)
  #    AGE_IDENTITY_B64          — alternative: base64-encoded identity
  #    HIMITSU_ENCRYPTED_DIR     — where .age files live (default: baked or /run/himitsu/secrets)
  #    HIMITSU_SECRETS_DIR       — where plaintext is written (default /run/secrets)
  #    HIMITSU_EXPORT_ENV        — "1" to export as env vars (default "1")
  #    HIMITSU_SERVER            — "1" to start credential server (default "0")
  #    HIMITSU_SERVER_PORT       — server port (default 9292)
  #    HIMITSU_SERVER_HOST       — server bind addr (default 0.0.0.0)
  #

  mkEntrypoint =
    {
      secrets ? null, # baked-in packSecrets derivation (optional)
      secretsPath ? "/run/himitsu/secrets", # fallback when no baked secrets
      exportEnv ? true,
      credentialServer ? {
        enable = false;
        port = 9292;
      },
    }:
    let
      defaultEncDir = if secrets != null then "${secrets}/secrets" else secretsPath;

      defaultExportEnv = if exportEnv then "1" else "0";
      defaultServer = if (credentialServer.enable or false) then "1" else "0";
      defaultPort = toString (credentialServer.port or 9292);
    in
    pkgs.writeShellScriptBin "himitsu-entrypoint" ''
      set -euo pipefail

      # ── Resolve age identity ───────────────────────────────────────
      if [ -n "''${AGE_IDENTITY_B64:-}" ]; then
        _idfile="$(mktemp)"
        echo "$AGE_IDENTITY_B64" | base64 -d > "$_idfile"
        chmod 600 "$_idfile"
        IDENTITY="$_idfile"
      else
        IDENTITY="''${HIMITSU_IDENTITY:-/identity/age.txt}"
      fi

      if [ ! -f "$IDENTITY" ]; then
        echo "fatal: no age identity found." >&2
        echo "  Mount at /identity/age.txt, set HIMITSU_IDENTITY, or set AGE_IDENTITY_B64." >&2
        exit 1
      fi

      # ── Decrypt ────────────────────────────────────────────────────
      ENCRYPTED_DIR="''${HIMITSU_ENCRYPTED_DIR:-${defaultEncDir}}"
      SECRETS_DIR="''${HIMITSU_SECRETS_DIR:-/run/secrets}"
      mkdir -p "$SECRETS_DIR"
      chmod 700 "$SECRETS_DIR"

      _count=0
      for f in "$ENCRYPTED_DIR"/*.age; do
        [ -f "$f" ] || continue
        name="$(basename "$f" .age)"
        if ${age}/bin/age --decrypt -i "$IDENTITY" -o "$SECRETS_DIR/$name" "$f" 2>/dev/null; then
          chmod 600 "$SECRETS_DIR/$name"
          _count=$(( _count + 1 ))
        else
          echo "warning: failed to decrypt $name" >&2
        fi
      done
      echo "himitsu: decrypted $_count secret(s)" >&2

      # ── Export env vars ────────────────────────────────────────────
      if [ "''${HIMITSU_EXPORT_ENV:-${defaultExportEnv}}" = "1" ]; then
        for f in "$SECRETS_DIR"/*; do
          [ -f "$f" ] || continue
          name="$(basename "$f")"
          IFS= read -r -d "" val < "$f" || true
          export "$name=$val"
        done
      fi

      # ── Credential server ──────────────────────────────────────────
      if [ "''${HIMITSU_SERVER:-${defaultServer}}" = "1" ]; then
        _port="''${HIMITSU_SERVER_PORT:-${defaultPort}}"
        _host="''${HIMITSU_SERVER_HOST:-0.0.0.0}"
        ${pkgs.python3}/bin/python3 ${credentialServerPy} \
          --secrets-dir "$SECRETS_DIR" \
          --port "$_port" \
          --host "$_host" \
          --identity-file "$IDENTITY" &
        echo "himitsu: credential server on $_host:$_port" >&2
      fi

      # ── Exec CMD ───────────────────────────────────────────────────
      exec "$@"
    '';

  # ===================================================================
  #  mkSecretsLayer
  # ===================================================================
  #
  #  Produce encrypted secrets as a container-ready artefact:
  #
  #    $out/secrets/*.age       — raw files (for dockerTools `contents`
  #                               or `COPY --from=`)
  #    $out/layer.tar           — tar rooted at `destPath` for use as
  #                               an OCI layer with skopeo/crane
  #    $out/manifest.txt        — key names
  #
  #  Usage with dockerTools:
  #
  #    dockerTools.buildLayeredImage {
  #      contents = [ (mkSecretsLayer { store = ./.himitsu; env = "prod"; }) ];
  #    }
  #
  #  Usage with Dockerfile:
  #
  #    # Build the layer:  nix build .#secretsLayer
  #    # Then:
  #    COPY --from=secrets-layer /secrets/ /run/himitsu/secrets/
  #
  #  Usage with skopeo / crane to append a layer:
  #
  #    crane append -f $(nix build .#secretsLayer --print-out-paths)/layer.tar \
  #      -t ghcr.io/org/app:with-secrets
  #

  mkSecretsLayer =
    {
      secrets ? null,
      store ? null,
      env ? "dev",
      mergeEnvs ? [ "common" ],
      destPath ? "run/himitsu/secrets", # path INSIDE the tar / container
      name ? "himitsu-secrets-layer",
    }:
    let
      resolved = resolveSecrets {
        inherit
          secrets
          store
          env
          mergeEnvs
          ;
      };
    in
    pkgs.runCommand name
      {
        preferLocalBuild = true;
      }
      ''
        mkdir -p $out/secrets

        # Copy .age files for direct consumption
        for f in ${resolved}/secrets/*.age; do
          [ -f "$f" ] && cp "$f" $out/secrets/ || true
        done

        cp ${resolved}/manifest.txt $out/manifest.txt

        # Build a tar rooted at the container dest path.
        # Extracting this at / places files at /${destPath}/*.age
        _stage="$(mktemp -d)"
        mkdir -p "$_stage/${destPath}"
        for f in ${resolved}/secrets/*.age; do
          [ -f "$f" ] && cp "$f" "$_stage/${destPath}/" || true
        done
        tar -cf $out/layer.tar -C "$_stage" .
        rm -rf "$_stage"
      '';

  # ===================================================================
  #  mkCredentialServerImage
  # ===================================================================
  #
  #  Minimal OCI image: encrypted secrets + entrypoint + age + python.
  #
  #  The age identity is NEVER baked into the image.  Provide it at
  #  runtime via volume mount or AGE_IDENTITY_B64 env var.
  #
  #    docker run -p 9292:9292 \
  #      -v ~/.himitsu/keys/age.txt:/identity/age.txt:ro \
  #      himitsu-credential-server
  #
  #    # — or —
  #    docker run -p 9292:9292 \
  #      -e AGE_IDENTITY_B64="$(base64 < ~/.himitsu/keys/age.txt)" \
  #      himitsu-credential-server
  #

  mkCredentialServerImage =
    {
      secrets ? null,
      store ? null,
      env ? "dev",
      mergeEnvs ? [ "common" ],
      port ? 9292,
      name ? "himitsu-credential-server",
      tag ? "latest",
    }:
    let
      resolved = resolveSecrets {
        inherit
          secrets
          store
          env
          mergeEnvs
          ;
      };

      entrypoint = mkEntrypoint {
        secrets = resolved;
        exportEnv = false;
        credentialServer = {
          enable = true;
          inherit port;
        };
      };
    in
    pkgs.dockerTools.buildLayeredImage {
      inherit name tag;
      contents = [
        pkgs.coreutils
        pkgs.bashInteractive
        age
        pkgs.python3
        resolved
        entrypoint
      ];
      config = {
        Entrypoint = [
          "${entrypoint}/bin/himitsu-entrypoint"
        ];
        Cmd = [
          "${pkgs.coreutils}/bin/sleep"
          "infinity"
        ];
        ExposedPorts = {
          "${toString port}/tcp" = { };
        };
        Env = [
          "HIMITSU_SERVER=1"
          "HIMITSU_SERVER_PORT=${toString port}"
          "HIMITSU_SERVER_HOST=0.0.0.0"
          "HIMITSU_EXPORT_ENV=0"
        ];
        Labels = {
          "dev.himitsu.component" = "credential-server";
          "dev.himitsu.env" = env;
        };
      };
    };

in
{
  inherit
    packSecrets
    mkDevShell
    wrapAge
    wrapSops
    mkCredentialServer
    mkEntrypoint
    mkSecretsLayer
    mkCredentialServerImage
    ;
}
