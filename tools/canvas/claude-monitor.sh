#!/bin/bash
# Claude Code Monitor — Canvas driver
# POST once, Splash handles tabs via on_click + set_text
# Re-run to refresh data

set -eo pipefail

for cmd in jq curl; do
  command -v "$cmd" &>/dev/null || { echo "Error: $cmd not found"; exit 1; }
done

HAS_TOKSCALE=0
command -v tokscale &>/dev/null && HAS_TOKSCALE=1

PORT=$(cat /tmp/makepad-canvas.port 2>/dev/null || true)
[ -z "$PORT" ] && { echo "Canvas not running."; exit 1; }
API="http://127.0.0.1:$PORT"

fmt_num() {
  local n=$1
  if [ "$n" -ge 1000000 ]; then echo "$((n/1000000)).$((n%1000000/100000))M"
  elif [ "$n" -ge 1000 ]; then echo "$((n/1000)).$((n%1000/100))K"
  else echo "$n"; fi
}

# ── Gather session data ──
SESSION_FILE=$(ls -t ~/.claude/projects/*/*.jsonl 2>/dev/null | head -1 || true)
S_INPUT=0; S_OUTPUT=0; S_CACHE_R=0; S_CACHE_W=0
S_HUMAN=0; S_ASSISTANT=0; S_TOOL_USE=0
MODEL="unknown"; TOOLS_TEXT="No tool usage"; AGENTS_TEXT="No agents"

if [ -n "$SESSION_FILE" ] && [ -f "$SESSION_FILE" ]; then
  TJ=$(tail -n 1000 "$SESSION_FILE" | jq -s '[.[]|select(.type=="assistant")|.message.usage//empty]|{i:(map(.input_tokens//0)|add//0),o:(map(.output_tokens//0)|add//0),cr:(map(.cache_read_input_tokens//0)|add//0),cw:(map(.cache_creation_input_tokens//0)|add//0)}' 2>/dev/null || echo '{"i":0,"o":0,"cr":0,"cw":0}')
  S_INPUT=$(echo "$TJ"|jq '.i'); S_OUTPUT=$(echo "$TJ"|jq '.o')
  S_CACHE_R=$(echo "$TJ"|jq '.cr'); S_CACHE_W=$(echo "$TJ"|jq '.cw')

  MJ=$(tail -n 1000 "$SESSION_FILE" | jq -s '{h:[.[]|select(.type=="user")]|length,a:[.[]|select(.type=="assistant")]|length,t:[.[]|select(.type=="assistant")|.message.content[]?|select(.type=="tool_use")]|length}' 2>/dev/null || echo '{"h":0,"a":0,"t":0}')
  S_HUMAN=$(echo "$MJ"|jq '.h'); S_ASSISTANT=$(echo "$MJ"|jq '.a'); S_TOOL_USE=$(echo "$MJ"|jq '.t')

  MODEL=$(tail -n 200 "$SESSION_FILE" | jq -r 'select(.type=="assistant")|.message.model//empty' 2>/dev/null | tail -1)
  [ -z "$MODEL" ] && MODEL="unknown"

  # Tools summary as text lines
  TOOLS_TEXT=$(tail -n 1000 "$SESSION_FILE" | jq -rs '[.[]|select(.type=="assistant")|.message.content[]?|select(.type=="tool_use")|.name//empty]|group_by(.)|map(.[0]+" "+((.|length)|tostring))|sort|reverse|.[0:8]|join("  |  ")' 2>/dev/null || echo "No tool usage")
  [ -z "$TOOLS_TEXT" ] && TOOLS_TEXT="No tool usage"

  # Agents summary
  AGENTS_TEXT=$(tail -n 500 "$SESSION_FILE" | jq -rs '[.[]|select(.type=="assistant")|.message.content[]?|select(.type=="tool_use")|select(.name=="Agent")|(.input.subagent_type//"general")+": "+(.input.description//"?")]|.[-4:]|join("  |  ")' 2>/dev/null || echo "No agents")
  [ -z "$AGENTS_TEXT" ] && AGENTS_TEXT="No agents"
fi

SI=$(fmt_num "$S_INPUT"); SO=$(fmt_num "$S_OUTPUT"); SCR=$(fmt_num "$S_CACHE_R"); SCW=$(fmt_num "$S_CACHE_W")
CTX=$((S_INPUT+S_CACHE_R)); CTX_PCT=$((CTX*100/1000000))
[ "$CTX_PCT" -gt 100 ] && CTX_PCT=100
CTX_FMT=$(fmt_num "$CTX")
BAR_W=$((CTX_PCT*4)); [ "$BAR_W" -lt 1 ] && BAR_W=1; [ "$BAR_W" -gt 400 ] && BAR_W=400
BAR_C="#x3366aa"
[ "$CTX_PCT" -gt 75 ] && BAR_C="#xff6b6b"
[ "$CTX_PCT" -gt 50 ] && [ "$CTX_PCT" -le 75 ] && BAR_C="#xffaa66"

# ── API equivalent cost calculator (jq) ──
# Prices per million tokens: opus=$15/$75/$1.875/$18.75, sonnet=$3/$15/$0.375/$3.75, haiku=$0.80/$4/$0.08/$1
calc_api_cost() {
  echo "$1" | jq '[.entries[] | (
    if (.model | test("opus")) then
      (.input//0)*15 + (.output//0)*75 + (.cacheRead//0)*1.875 + (.cacheWrite//0)*18.75
    elif (.model | test("sonnet")) then
      (.input//0)*3 + (.output//0)*15 + (.cacheRead//0)*0.375 + (.cacheWrite//0)*3.75
    elif (.model | test("haiku")) then
      (.input//0)*0.80 + (.output//0)*4 + (.cacheRead//0)*0.08 + (.cacheWrite//0)*1
    else 0 end
  ) / 1000000] | add // 0 | . * 100 | floor | . / 100' 2>/dev/null || echo "0"
}

# Summary line from tokscale JSON
summarize_ts() {
  local js=$1 period=$2
  local ti=$(echo "$js"|jq '.totalInput//0')
  local to=$(echo "$js"|jq '.totalOutput//0')
  local tm=$(echo "$js"|jq '.totalMessages//0')
  local cost=$(calc_api_cost "$js")
  echo "$period: $(fmt_num "$ti") in / $(fmt_num "$to") out | ${tm} msgs | ~\$${cost}"
}

# ── Gather tokscale data (day/week/month) ──
STATS_DAY=""; STATS_WEEK=""; STATS_MONTH=""
STATS_MODELS=""; STATS_DAY_COST="N/A"
D_TI=0; D_TO=0; D_TM=0

if [ "$HAS_TOKSCALE" -eq 1 ]; then
  TS_DAY=$(tokscale models --claude --json --today --no-spinner 2>/dev/null || echo '{}')
  TS_WEEK=$(tokscale models --claude --json --week --no-spinner 2>/dev/null || echo '{}')
  TS_MONTH=$(tokscale models --claude --json --month --no-spinner 2>/dev/null || echo '{}')

  STATS_DAY=$(summarize_ts "$TS_DAY" "Today")
  STATS_WEEK=$(summarize_ts "$TS_WEEK" "Week")
  STATS_MONTH=$(summarize_ts "$TS_MONTH" "Month")

  D_TI=$(echo "$TS_DAY"|jq '.totalInput//0')
  D_TO=$(echo "$TS_DAY"|jq '.totalOutput//0')
  D_TM=$(echo "$TS_DAY"|jq '.totalMessages//0')
  STATS_DAY_COST="\$$(calc_api_cost "$TS_DAY")"

  STATS_MODELS=$(echo "$TS_DAY" | jq -r '[.entries[]|select(.model!="<synthetic>")|(.model|sub("claude-";""))+" "+(((.input//0)+(.output//0))*100/(([.input//0,.output//0]|add)|if .==0 then 1 else . end)| floor|tostring)+"%"]|join("  ")' 2>/dev/null || echo "")
  # Recalculate with total
  TTOK=$((D_TI+D_TO)); [ "$TTOK" -lt 1 ] && TTOK=1
  STATS_MODELS=$(echo "$TS_DAY" | jq -r --argjson tot "$TTOK" '[.entries[]|select(.model!="<synthetic>")|(.model|sub("claude-";""))+" "+((((.input//0)+(.output//0))*100/$tot)|floor|tostring)+"%"]|join("  |  ")' 2>/dev/null || echo "No data")
  [ -z "$STATS_MODELS" ] && STATS_MODELS="No data"
else
  STATS_DAY="tokscale not installed"
  STATS_WEEK=""
  STATS_MONTH=""
fi

# ── Escape for Splash strings ──
esc() { echo "$1" | sed 's/"/\\"/g'; }

# ── Build Splash ──
# Uses the beautiful card layout from examples/claude-monitor.splash
# All value labels are named with := for dynamic set_text updates
TMPFILE="/tmp/claude-monitor-splash.$$.tmp"
cat > "$TMPFILE" <<SPLASHEOF
let state = { tab: "session" elapsed: 0 }

fn show_session() {
    state.tab = "session"
    ui.tab_s.set_text("> Session")
    ui.tab_t.set_text("Tools")
    ui.tab_x.set_text("Stats")
    ui.c1t.set_text("Input")
    ui.c1v.set_text("$SI")
    ui.c2t.set_text("Output")
    ui.c2v.set_text("$SO")
    ui.c3t.set_text("Cache Read")
    ui.c3v.set_text("$SCR")
    ui.c4t.set_text("Cache Write")
    ui.c4v.set_text("$SCW")
    ui.ctx_label.set_text("$CTX_FMT / 1M  (${CTX_PCT}%)")
    ui.sec_title.set_text("Session Activity")
    ui.d1t.set_text("Human")
    ui.d1v.set_text("$S_HUMAN")
    ui.d2t.set_text("Assistant")
    ui.d2v.set_text("$S_ASSISTANT")
    ui.d3t.set_text("Tool Use")
    ui.d3v.set_text("$S_TOOL_USE")
    ui.d4t.set_text("Session")
    ui.d4v.set_text(fmt_elapsed())
}

fn show_tools() {
    state.tab = "tools"
    ui.tab_s.set_text("Session")
    ui.tab_t.set_text("> Tools")
    ui.tab_x.set_text("Stats")
    ui.c1t.set_text("Top Tool")
    ui.c1v.set_text("$(echo "$TOOL_JSON"|jq -r '.[0].n//"--"')")
    ui.c2t.set_text("Uses")
    ui.c2v.set_text("$(echo "$TOOL_JSON"|jq -r '.[0].c//0')")
    ui.c3t.set_text("Total Calls")
    ui.c3v.set_text("$S_TOOL_USE")
    ui.c4t.set_text("Agents")
    ui.c4v.set_text("$(echo "$AGENT_JSON"|jq 'length')")
    ui.ctx_label.set_text("$(esc "$TOOLS_TEXT")")
    ui.sec_title.set_text("Agents")
    ui.d1t.set_text("$(esc "$AGENTS_TEXT")")
    ui.d1v.set_text("")
    ui.d2t.set_text("")
    ui.d2v.set_text("")
    ui.d3t.set_text("")
    ui.d3v.set_text("")
    ui.d4t.set_text("")
    ui.d4v.set_text("")
}

fn show_stats() {
    state.tab = "stats"
    ui.tab_s.set_text("Session")
    ui.tab_t.set_text("Tools")
    ui.tab_x.set_text("> Stats")
    ui.c1t.set_text("Today In")
    ui.c1v.set_text("$([ "$HAS_TOKSCALE" -eq 1 ] && fmt_num "$D_TI" || echo "N/A")")
    ui.c2t.set_text("Today Out")
    ui.c2v.set_text("$([ "$HAS_TOKSCALE" -eq 1 ] && fmt_num "$D_TO" || echo "N/A")")
    ui.c3t.set_text("Messages")
    ui.c3v.set_text("$([ "$HAS_TOKSCALE" -eq 1 ] && echo "$D_TM" || echo "N/A")")
    ui.c4t.set_text("API Equiv")
    ui.c4v.set_text("$([ "$HAS_TOKSCALE" -eq 1 ] && echo "$STATS_DAY_COST" || echo "N/A")")
    ui.ctx_label.set_text("$(esc "$STATS_MODELS")")
    ui.sec_title.set_text("Usage by Period")
    ui.d1t.set_text("$(esc "$STATS_DAY")")
    ui.d1v.set_text("")
    ui.d2t.set_text("$(esc "$STATS_WEEK")")
    ui.d2v.set_text("")
    ui.d3t.set_text("$(esc "$STATS_MONTH")")
    ui.d3v.set_text("")
    ui.d4t.set_text("")
    ui.d4v.set_text("")
}

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
    if state.tab == "session" {
        ui.d4v.set_text(fmt_elapsed())
    }
}

SolidView{width: Fill height: Fit draw_bg.color: #x0c0c18 flow: Down padding: Inset{left: 32. right: 32. top: 24. bottom: 24.} spacing: 16

    // Header
    View{width: Fill height: Fit flow: Right align: Align{y: 0.5}
        Label{text: "Claude Code Monitor" draw_text.color: #xeeeeff draw_text.text_style.font_size: 20}
        Filler{}
        Label{text: "$MODEL" draw_text.color: #xcc66ff draw_text.text_style.font_size: 11}
    }

    // Tab buttons
    View{width: Fill height: Fit flow: Right spacing: 8
        tab_s := Button{text: "> Session" draw_bg.color: #x222244 draw_text.color: #xeeeeff padding: Inset{left: 16. right: 16. top: 8. bottom: 8.} draw_bg.radius: 6. width: 90 height: 36
            on_click: ||{ show_session() }
        }
        tab_t := Button{text: "Tools" draw_bg.color: #x222244 draw_text.color: #x8888aa padding: Inset{left: 16. right: 16. top: 8. bottom: 8.} draw_bg.radius: 6. width: 70 height: 36
            on_click: ||{ show_tools() }
        }
        tab_x := Button{text: "Stats" draw_bg.color: #x222244 draw_text.color: #x8888aa padding: Inset{left: 16. right: 16. top: 8. bottom: 8.} draw_bg.radius: 6. width: 70 height: 36
            on_click: ||{ show_stats() }
        }
    }

    // 4 metric cards
    View{width: Fill height: Fit flow: Right spacing: 12
        RoundedView{width: Fill height: Fit draw_bg.color: #x161628 draw_bg.radius: 8. padding: Inset{left: 16. right: 16. top: 14. bottom: 14.} flow: Down spacing: 4
            c1t := Label{text: "Input" draw_text.color: #x888899 draw_text.text_style.font_size: 9}
            c1v := Label{text: "$SI" draw_text.color: #xcc66ff draw_text.text_style.font_size: 24}
        }
        RoundedView{width: Fill height: Fit draw_bg.color: #x161628 draw_bg.radius: 8. padding: Inset{left: 16. right: 16. top: 14. bottom: 14.} flow: Down spacing: 4
            c2t := Label{text: "Output" draw_text.color: #x888899 draw_text.text_style.font_size: 9}
            c2v := Label{text: "$SO" draw_text.color: #x66aaff draw_text.text_style.font_size: 24}
        }
        RoundedView{width: Fill height: Fit draw_bg.color: #x161628 draw_bg.radius: 8. padding: Inset{left: 16. right: 16. top: 14. bottom: 14.} flow: Down spacing: 4
            c3t := Label{text: "Cache Read" draw_text.color: #x888899 draw_text.text_style.font_size: 9}
            c3v := Label{text: "$SCR" draw_text.color: #x66ffaa draw_text.text_style.font_size: 24}
        }
        RoundedView{width: Fill height: Fit draw_bg.color: #x161628 draw_bg.radius: 8. padding: Inset{left: 16. right: 16. top: 14. bottom: 14.} flow: Down spacing: 4
            c4t := Label{text: "Cache Write" draw_text.color: #x888899 draw_text.text_style.font_size: 9}
            c4v := Label{text: "$SCW" draw_text.color: #xffaa66 draw_text.text_style.font_size: 24}
        }
    }

    // Context / info bar
    RoundedView{width: Fill height: Fit draw_bg.color: #x161628 draw_bg.radius: 8. padding: Inset{left: 20. right: 20. top: 14. bottom: 14.} flow: Down spacing: 8
        View{width: Fill height: Fit flow: Right align: Align{y: 0.5}
            Label{text: "Context Window" draw_text.color: #xaaaacc draw_text.text_style.font_size: 11}
            Filler{}
            ctx_label := Label{text: "$CTX_FMT / 1M  (${CTX_PCT}%)" draw_text.color: #x888899 draw_text.text_style.font_size: 10}
        }
        View{width: Fill height: 8 flow: Overlay
            RoundedView{width: Fill height: Fill draw_bg.color: #x222233 draw_bg.radius: 4.}
            RoundedView{width: $BAR_W height: Fill draw_bg.color: $BAR_C draw_bg.radius: 4.}
        }
    }

    // Detail section (4 stat pairs in a row)
    RoundedView{width: Fill height: Fit draw_bg.color: #x161628 draw_bg.radius: 8. padding: Inset{left: 20. right: 20. top: 14. bottom: 14.} flow: Down spacing: 8
        sec_title := Label{text: "Session Activity" draw_text.color: #xaaaacc draw_text.text_style.font_size: 11}
        View{width: Fill height: Fit flow: Right spacing: 24
            View{width: Fit height: Fit flow: Down spacing: 2
                d1t := Label{text: "Human" draw_text.color: #x666688 draw_text.text_style.font_size: 9}
                d1v := Label{text: "$S_HUMAN" draw_text.color: #xffaa66 draw_text.text_style.font_size: 20}
            }
            View{width: Fit height: Fit flow: Down spacing: 2
                d2t := Label{text: "Assistant" draw_text.color: #x666688 draw_text.text_style.font_size: 9}
                d2v := Label{text: "$S_ASSISTANT" draw_text.color: #x66aaff draw_text.text_style.font_size: 20}
            }
            View{width: Fit height: Fit flow: Down spacing: 2
                d3t := Label{text: "Tool Use" draw_text.color: #x666688 draw_text.text_style.font_size: 9}
                d3v := Label{text: "$S_TOOL_USE" draw_text.color: #xcc66ff draw_text.text_style.font_size: 20}
            }
            Filler{}
            View{width: Fit height: Fit flow: Down spacing: 2
                d4t := Label{text: "Session" draw_text.color: #x666688 draw_text.text_style.font_size: 9}
                d4v := Label{text: "00:00:00" draw_text.color: #xeeeeff draw_text.text_style.font_size: 20}
            }
        }
    }
}
SPLASHEOF

RESULT=$(curl -s -X POST "$API/splash" --data-binary "@$TMPFILE" 2>&1)
rm -f "$TMPFILE"

if echo "$RESULT" | grep -q '"ok":true'; then
  echo "Claude Code Monitor loaded (port $PORT)"
  echo "  $MODEL | $SI in / $SO out | $S_HUMAN human / $S_ASSISTANT asst / $S_TOOL_USE tools"
  echo "  Click Session/Tools/Stats tabs. Timer auto-ticks. Re-run to refresh."
else
  echo "Error: $RESULT"
  exit 1
fi
