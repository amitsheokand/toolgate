# Shared hooks.json / Muse settings merge (stable toolgate identity by harness+event).
{ pkgs, toolgateBasename ? "toolgate" }:
pkgs.writeShellScript "toolgate-merge-hooks" ''
  set -euo pipefail
  target="$1"
  shift
  tmp=$(mktemp)
  if [ -f "$target" ]; then
    cp "$target" "$tmp"
  else
    echo '{}' > "$tmp"
  fi
  merge_jq='
    def command_key(e):
      if (e | type) != "object" then null
      elif e.command? then e.command
      elif e.hooks? and (e.hooks | length) > 0 then e.hooks[0].command
      else null end;
    def toolgate_stable_key($bin; $cmd):
      if ($cmd | type) != "string" then null
      elif ($cmd | contains($bin)) and ($cmd | contains("hook --harness")) then
        ($cmd | capture("(?<s>hook --harness .+)").s)
      else null end;
    def entry_toolgate_key($bin; e):
      command_key(e) as $c | toolgate_stable_key($bin; $c);
    def is_toolgate_entry($bin; e):
      entry_toolgate_key($bin; e) != null;
    def merge_lists($bin; $base; $patch):
      ($base // []) as $b |
      ($b | map(select(is_toolgate_entry($bin; .) | not))) as $peers |
      if $patch == null then $peers
      else $peers + $patch end;
    def merge_hooks($bin; $base; $patch):
      ($base.hooks // {}) as $bh | ($patch.hooks // {}) as $ph |
      reduce (($bh | keys) + ($ph | keys) | unique | .[]) as $k
        ({}; . + { ($k): merge_lists($bin; $bh[$k]; $ph[$k]) }) as $merged |
      $base * $patch | .hooks = $merged;
    merge_hooks($bin; .[0]; .[1])
  '
  for patch in "$@"; do
    ${pkgs.jq}/bin/jq -s --arg bin "${toolgateBasename}" "$merge_jq" "$tmp" "$patch" > "$tmp.new"
    mv "$tmp.new" "$tmp"
  done
  mkdir -p "$(dirname "$target")"
  mv "$tmp" "$target"
''
