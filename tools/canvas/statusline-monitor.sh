#!/bin/bash
# Claude Code Statusline → Canvas Monitor
# Configure in ~/.claude/settings.json:
#   "statusLine": { "type": "command", "command": "~/.claude/statusline-monitor.sh" }
#
# Reads Claude Code statusline JSON from stdin, updates Canvas monitor.
# Also outputs a compact statusline string for Claude Code's bottom bar.

set -eo pipefail

# Read JSON from stdin (Claude Code pushes this after each message)
INPUT=$(cat)

# Extract fields
MODEL=$(echo "$INPUT" | jq -r '.model.display_name // "Unknown"' 2>/dev/null)
MODEL_ID=$(echo "$INPUT" | jq -r '.model.id // "unknown"' 2>/dev/null)
TI=$(echo "$INPUT" | jq -r '.context_window.total_input_tokens // 0' 2>/dev/null)
TO=$(echo "$INPUT" | jq -r '.context_window.total_output_tokens // 0' 2>/dev/null)
CW=$(echo "$INPUT" | jq -r '.context_window.current_usage.cache_creation_input_tokens // 0' 2>/dev/null)
CR=$(echo "$INPUT" | jq -r '.context_window.current_usage.cache_read_input_tokens // 0' 2>/dev/null)
PCT=$(echo "$INPUT" | jq -r '.context_window.used_percentage // 0' 2>/dev/null | cut -d. -f1)
COST=$(echo "$INPUT" | jq -r '.cost.total_cost_usd // 0' 2>/dev/null)
DUR_MS=$(echo "$INPUT" | jq -r '.cost.total_duration_ms // 0' 2>/dev/null)
LINES_ADD=$(echo "$INPUT" | jq -r '.cost.total_lines_added // 0' 2>/dev/null)
LINES_DEL=$(echo "$INPUT" | jq -r '.cost.total_lines_removed // 0' 2>/dev/null)

# Workspace
CWD=$(echo "$INPUT" | jq -r '.workspace.current_dir // "unknown"' 2>/dev/null)
PROJECT_DIR=$(echo "$INPUT" | jq -r '.workspace.project_dir // ""' 2>/dev/null)
# Use project dir basename as project name, fallback to cwd basename
if [ -n "$PROJECT_DIR" ]; then
  PROJECT_NAME=$(basename "$PROJECT_DIR")
else
  PROJECT_NAME=$(basename "$CWD")
fi

# Rate limits
RL5_PCT=$(echo "$INPUT" | jq -r '.rate_limits.five_hour.used_percentage // 0' 2>/dev/null | cut -d. -f1)
RL7_PCT=$(echo "$INPUT" | jq -r '.rate_limits.seven_day.used_percentage // 0' 2>/dev/null | cut -d. -f1)

# Format numbers
fmt() {
  local n=$1
  if [ "$n" -ge 1000000 ] 2>/dev/null; then echo "$((n/1000000)).$((n%1000000/100000))M"
  elif [ "$n" -ge 1000 ] 2>/dev/null; then echo "$((n/1000)).$((n%1000/100))K"
  else echo "$n"; fi
}

# Format duration
DUR_S=$((DUR_MS/1000))
DUR_H=$((DUR_S/3600)); DUR_M=$(((DUR_S%3600)/60)); DUR_SEC=$((DUR_S%60))
DUR_FMT=$(printf "%02d:%02d:%02d" "$DUR_H" "$DUR_M" "$DUR_SEC")

SI=$(fmt "$TI"); SO=$(fmt "$TO"); SCR=$(fmt "$CR"); SCW=$(fmt "$CW")
CTX=$((TI+CR)); CTX_FMT=$(fmt "$CTX")

# Progress bar width (max 400px)
BAR_W=$((PCT*4)); [ "$BAR_W" -lt 1 ] && BAR_W=1; [ "$BAR_W" -gt 400 ] && BAR_W=400
BAR_C="#x3366aa"
[ "$PCT" -gt 75 ] && BAR_C="#xff6b6b"
[ "$PCT" -gt 50 ] && [ "$PCT" -le 75 ] && BAR_C="#xffaa66"

# Format cost
COST_FMT=$(printf "\$%.2f" "$COST" 2>/dev/null || echo "\$0.00")

# ── Update Canvas if running ──
PORT=$(cat /tmp/makepad-canvas.port 2>/dev/null || true)
if [ -n "$PORT" ] && curl -sf --max-time 1 "http://127.0.0.1:$PORT/ping" >/dev/null 2>&1; then
  TMPFILE="/tmp/canvas-statusline.$$.splash"
  cat > "$TMPFILE" << SPLASHEOF
let state = { elapsed: $DUR_S }

fn fmt_elapsed() {
    let t = state.elapsed
    let h = 0
    while t >= 3600 { h = h + 1  t = t - 3600 }
    let m = 0
    while t >= 60 { m = m + 1  t = t - 60 }
    let s = t
    let hh = if h < 10 { "0" + h } else { "" + h }
    let mm = if m < 10 { "0" + m } else { "" + m }
    let ss = if s < 10 { "0" + s } else { "" + s }
    hh + ":" + mm + ":" + ss
}

fn tick() {
    state.elapsed = state.elapsed + 1
    ui.dur_val.set_text(fmt_elapsed())
}

SolidView{width: Fill height: Fit draw_bg.color: #x0c0c18 flow: Down padding: Inset{left: 32. right: 32. top: 24. bottom: 24.} spacing: 16

    View{width: Fill height: Fit flow: Right align: Align{y: 0.5}
        Label{text: "Claude Code Monitor" draw_text.color: #xeeeeff draw_text.text_style.font_size: 20}
        Filler{}
        Label{text: "$PROJECT_NAME" draw_text.color: #x66ffaa draw_text.text_style.font_size: 11}
        View{width: 12}
        Label{text: "$MODEL_ID" draw_text.color: #xcc66ff draw_text.text_style.font_size: 11}
    }

    View{width: Fill height: Fit flow: Right spacing: 12
        RoundedView{width: Fill height: Fit draw_bg.color: #x161628 draw_bg.radius: 8. padding: Inset{left: 16. right: 16. top: 14. bottom: 14.} flow: Down spacing: 4
            Label{text: "Input" draw_text.color: #x888899 draw_text.text_style.font_size: 9}
            Label{text: "$SI" draw_text.color: #xcc66ff draw_text.text_style.font_size: 24}
        }
        RoundedView{width: Fill height: Fit draw_bg.color: #x161628 draw_bg.radius: 8. padding: Inset{left: 16. right: 16. top: 14. bottom: 14.} flow: Down spacing: 4
            Label{text: "Output" draw_text.color: #x888899 draw_text.text_style.font_size: 9}
            Label{text: "$SO" draw_text.color: #x66aaff draw_text.text_style.font_size: 24}
        }
        RoundedView{width: Fill height: Fit draw_bg.color: #x161628 draw_bg.radius: 8. padding: Inset{left: 16. right: 16. top: 14. bottom: 14.} flow: Down spacing: 4
            Label{text: "Cache Read" draw_text.color: #x888899 draw_text.text_style.font_size: 9}
            Label{text: "$SCR" draw_text.color: #x66ffaa draw_text.text_style.font_size: 24}
        }
        RoundedView{width: Fill height: Fit draw_bg.color: #x161628 draw_bg.radius: 8. padding: Inset{left: 16. right: 16. top: 14. bottom: 14.} flow: Down spacing: 4
            Label{text: "Cache Write" draw_text.color: #x888899 draw_text.text_style.font_size: 9}
            Label{text: "$SCW" draw_text.color: #xffaa66 draw_text.text_style.font_size: 24}
        }
    }

    RoundedView{width: Fill height: Fit draw_bg.color: #x161628 draw_bg.radius: 8. padding: Inset{left: 20. right: 20. top: 14. bottom: 14.} flow: Down spacing: 8
        View{width: Fill height: Fit flow: Right align: Align{y: 0.5}
            Label{text: "Context Window" draw_text.color: #xaaaacc draw_text.text_style.font_size: 11}
            Filler{}
            Label{text: "$CTX_FMT / 1M  (${PCT}%)" draw_text.color: #x888899 draw_text.text_style.font_size: 10}
        }
        View{width: Fill height: 8 flow: Overlay
            RoundedView{width: Fill height: Fill draw_bg.color: #x222233 draw_bg.radius: 4.}
            RoundedView{width: $BAR_W height: Fill draw_bg.color: $BAR_C draw_bg.radius: 4.}
        }
    }

    RoundedView{width: Fill height: Fit draw_bg.color: #x161628 draw_bg.radius: 8. padding: Inset{left: 20. right: 20. top: 14. bottom: 14.} flow: Down spacing: 8
        Label{text: "Session" draw_text.color: #xaaaacc draw_text.text_style.font_size: 11}
        View{width: Fill height: Fit flow: Right spacing: 24
            View{width: Fit height: Fit flow: Down spacing: 2
                Label{text: "Duration" draw_text.color: #x666688 draw_text.text_style.font_size: 9}
                dur_val := Label{text: "$DUR_FMT" draw_text.color: #xeeeeff draw_text.text_style.font_size: 20}
            }
            View{width: Fit height: Fit flow: Down spacing: 2
                Label{text: "Cost" draw_text.color: #x666688 draw_text.text_style.font_size: 9}
                Label{text: "$COST_FMT" draw_text.color: #x66ffaa draw_text.text_style.font_size: 20}
            }
            View{width: Fit height: Fit flow: Down spacing: 2
                Label{text: "Lines +/-" draw_text.color: #x666688 draw_text.text_style.font_size: 9}
                Label{text: "+$LINES_ADD / -$LINES_DEL" draw_text.color: #xffaa66 draw_text.text_style.font_size: 20}
            }
            Filler{}
            View{width: Fit height: Fit flow: Down spacing: 2
                Label{text: "Rate (5h/7d)" draw_text.color: #x666688 draw_text.text_style.font_size: 9}
                Label{text: "${RL5_PCT}% / ${RL7_PCT}%" draw_text.color: #x66aaff draw_text.text_style.font_size: 20}
            }
        }
    }
}
SPLASHEOF

  curl -sf -X POST "http://127.0.0.1:$PORT/splash" --data-binary "@$TMPFILE" >/dev/null 2>&1

  # Keep example in sync with latest real data
  SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
  cp -f "$TMPFILE" "$SCRIPT_DIR/examples/claude-monitor.splash" 2>/dev/null || true

  rm -f "$TMPFILE"
fi

# ── Output statusline text for Claude Code bottom bar ──
echo "[$MODEL] $PROJECT_NAME | IN:$SI OUT:$SO | ${PCT}% | $COST_FMT | $DUR_FMT"
