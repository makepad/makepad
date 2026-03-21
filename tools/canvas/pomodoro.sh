#!/bin/bash
# Pomodoro Timer — driven via Makepad Canvas HTTP API
# Usage: bash pomodoro.sh

PORT=$(cat /tmp/makepad-canvas.port 2>/dev/null)
if [ -z "$PORT" ]; then
  echo "Canvas not running. Start makepad-canvas first."
  exit 1
fi
API="http://127.0.0.1:$PORT"

# ── State ──
MODE="work"       # work | break | long_break
WORK_SECS=1500    # 25 min
BREAK_SECS=300    # 5 min
LONG_BREAK=900    # 15 min
REMAINING=$WORK_SECS
RUNNING=0
SESSIONS=0
PAUSED_AT=0

# ── Sound ──
play_bell() {
  # Play completion sound (3x chime for work end, 1x for break end)
  local count=${1:-1}
  for ((i=0; i<count; i++)); do
    afplay /System/Library/Sounds/Glass.aiff &
    sleep 0.6
  done
}

play_tick() {
  # Subtle tick for last 10 seconds
  afplay /System/Library/Sounds/Tink.aiff &
}

play_start() {
  afplay /System/Library/Sounds/Pop.aiff &
}

# ── Formatting ──
fmt_time() {
  local m=$(( $1 / 60 ))
  local s=$(( $1 % 60 ))
  printf "%02d:%02d" $m $s
}

progress_bar() {
  local total=$1
  local remaining=$2
  local elapsed=$(( total - remaining ))
  local pct=0
  if [ $total -gt 0 ]; then
    pct=$(( elapsed * 100 / total ))
  fi
  echo $pct
}

mode_label() {
  case $MODE in
    work)       echo "FOCUS TIME" ;;
    break)      echo "SHORT BREAK" ;;
    long_break) echo "LONG BREAK" ;;
  esac
}

mode_color() {
  case $MODE in
    work)       echo "#xff6b6b" ;;
    break)      echo "#x51cf66" ;;
    long_break) echo "#x339af0" ;;
  esac
}

mode_bg() {
  case $MODE in
    work)       echo "#x2a1a1a" ;;
    break)      echo "#x1a2a1a" ;;
    long_break) echo "#x1a1a2a" ;;
  esac
}

mode_total() {
  case $MODE in
    work)       echo $WORK_SECS ;;
    break)      echo $BREAK_SECS ;;
    long_break) echo $LONG_BREAK ;;
  esac
}

session_dots() {
  local dots=""
  for ((i=1; i<=4; i++)); do
    if [ $i -le $SESSIONS ]; then
      dots="$dots 🍅"
    else
      dots="$dots ⚪"
    fi
  done
  echo "$dots"
}

# ── Render ──
render() {
  local time_str=$(fmt_time $REMAINING)
  local total=$(mode_total)
  local pct=$(progress_bar $total $REMAINING)
  local label=$(mode_label)
  local color=$(mode_color)
  local bg=$(mode_bg)
  local dots=$(session_dots)
  local btn_label="Start"
  local btn_color="#x51cf66"
  if [ $RUNNING -eq 1 ]; then
    btn_label="Pause"
    btn_color="#xffa94d"
  fi

  # Progress bar: filled portion
  local bar_width=400
  local filled=$(( bar_width * pct / 100 ))
  if [ $filled -lt 1 ]; then filled=1; fi

  curl -s -X POST "$API/splash" -d "View{width: Fill height: Fill flow: Down align: Center
    padding: Inset{left: 40. right: 40. top: 60. bottom: 40.}

    // Background
    SolidView{width: Fill height: Fill draw_bg.color: #x0a0a12 flow: Down align: Center spacing: 20
        padding: Inset{left: 40. right: 40. top: 50. bottom: 40.}

        // Session dots
        View{width: Fit height: Fit flow: Right spacing: 8 align: Center
            Label{text: \"$dots\" draw_text.text_style.font_size: 16}
        }

        // Mode label
        Label{text: \"$label\" draw_text.color: $color draw_text.text_style.font_size: 14}

        // Big timer
        Label{text: \"$time_str\" draw_text.color: #xffffff draw_text.text_style.font_size: 64}

        // Progress bar background
        View{width: 400 height: 8 flow: Overlay
            RoundedView{width: Fill height: Fill draw_bg.color: #x222233 draw_bg.radius: 4.}
            RoundedView{width: $filled height: Fill draw_bg.color: $color draw_bg.radius: 4.}
        }

        // Percentage
        Label{text: \"${pct}%\" draw_text.color: #x666688 draw_text.text_style.font_size: 11}

        // Spacer
        View{height: 20}

        // Buttons row
        View{width: Fit height: Fit flow: Right spacing: 16 align: Center

            // Start / Pause
            start_btn := Button{text: \"$btn_label\"
                draw_bg.color: $btn_color
                draw_text.color: #x111111
                padding: Inset{left: 24. right: 24. top: 12. bottom: 12.}
                draw_bg.radius: 6.
            }

            // Reset
            reset_btn := Button{text: \"Reset\"
                draw_bg.color: #x444466
                draw_text.color: #xccccdd
                padding: Inset{left: 24. right: 24. top: 12. bottom: 12.}
                draw_bg.radius: 6.
            }

            // Skip
            skip_btn := Button{text: \"Skip ⏭\"
                draw_bg.color: #x333355
                draw_text.color: #x9999bb
                padding: Inset{left: 24. right: 24. top: 12. bottom: 12.}
                draw_bg.radius: 6.
            }
        }

        // Spacer
        View{height: 10}

        // Info
        View{width: Fit height: Fit flow: Right spacing: 20
            Label{text: \"🍅 25 min\" draw_text.color: #xff6b6b draw_text.text_style.font_size: 11}
            Label{text: \"☕ 5 min\" draw_text.color: #x51cf66 draw_text.text_style.font_size: 11}
            Label{text: \"🌴 15 min\" draw_text.color: #x339af0 draw_text.text_style.font_size: 11}
        }
    }
}" > /dev/null
}

# ── Mode transitions ──
next_mode() {
  case $MODE in
    work)
      SESSIONS=$(( SESSIONS + 1 ))
      play_bell 3
      if [ $SESSIONS -ge 4 ]; then
        MODE="long_break"
        REMAINING=$LONG_BREAK
        SESSIONS=0
      else
        MODE="break"
        REMAINING=$BREAK_SECS
      fi
      ;;
    break|long_break)
      play_bell 1
      MODE="work"
      REMAINING=$WORK_SECS
      ;;
  esac
  RUNNING=0
}

reset_timer() {
  RUNNING=0
  case $MODE in
    work)       REMAINING=$WORK_SECS ;;
    break)      REMAINING=$BREAK_SECS ;;
    long_break) REMAINING=$LONG_BREAK ;;
  esac
}

skip_timer() {
  next_mode
  RUNNING=0
}

# ── Event polling (non-blocking) ──
check_event() {
  local resp
  resp=$(curl -s -m 1 "$API/event" 2>/dev/null)
  if [ -n "$resp" ] && [ "$resp" != "" ]; then
    local widget=$(echo "$resp" | grep -o '"widget":"[^"]*"' | head -1 | cut -d'"' -f4)
    case "$widget" in
      start_btn)
        if [ $RUNNING -eq 0 ]; then
          RUNNING=1
          play_start
        else
          RUNNING=0
        fi
        ;;
      reset_btn)
        reset_timer
        play_start
        ;;
      skip_btn)
        skip_timer
        ;;
    esac
  fi
}

# ── Main loop ──
echo "🍅 Pomodoro Timer started on Canvas (port $PORT)"
echo "   Press Ctrl+C to stop"

render

LAST_TICK=$(date +%s)

while true; do
  # Check for button events (non-blocking, 1s timeout matches our tick)
  check_event

  NOW=$(date +%s)
  ELAPSED=$(( NOW - LAST_TICK ))

  if [ $RUNNING -eq 1 ] && [ $ELAPSED -ge 1 ]; then
    REMAINING=$(( REMAINING - ELAPSED ))
    LAST_TICK=$NOW

    # Tick sound for last 5 seconds
    if [ $REMAINING -le 5 ] && [ $REMAINING -gt 0 ]; then
      play_tick
    fi

    if [ $REMAINING -le 0 ]; then
      REMAINING=0
      render
      sleep 0.5
      next_mode
    fi

    render
  elif [ $RUNNING -eq 0 ]; then
    LAST_TICK=$NOW
    render
  fi
done
