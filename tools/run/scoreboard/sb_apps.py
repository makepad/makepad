"""Interactive scoreboard workloads. Only drive the process tree owned by Ctx.

Remote requests and their frame barriers are part of each operation window;
there is no per-frame sampling or work injected into the application's UI loop.
"""
import concurrent.futures
import json
import os
import time


def objects(text):
    for line in text.splitlines():
        try:
            yield json.loads(line)
        except ValueError:
            continue


def walk(value):
    if isinstance(value, dict):
        yield value
        for child in value.values():
            yield from walk(child)
    elif isinstance(value, list):
        for child in value:
            yield from walk(child)


class Remote:
    def __init__(self, ctx):
        self.c = ctx
        self.seq = None

    def ask(self, *args):
        if not self.c.alive():
            raise RuntimeError('app exited or launch cap reached')
        reply = self.c.inspect(args, timeout=min(60, max(1, self.c.cap - self.c.now())))
        parsed = list(objects(reply))
        if not parsed:
            raise RuntimeError('inspect %s: %s' % (args, reply[-500:]))
        for obj in parsed:
            if isinstance(obj, dict) and 'err' in obj:
                raise RuntimeError('inspect %s: %s' % (args, obj))
        return parsed

    def sequence(self):
        for obj in walk(self.ask('activity')):
            if 'user_seq' in obj:
                self.seq = obj['user_seq']
                return self.seq
        raise RuntimeError('activity did not report user_seq')

    def input(self, *args, wait=True):
        if self.seq is None:
            self.sequence()
        tail = ['if_user_seq=%s' % self.seq]
        if wait:
            tail.append('wait=1')
        return self.ask(*(list(args) + tail))

    def entries(self, query=''):
        args = ['ui', 'find', 'json=1'] + (['q=' + query] if query else [])
        return [o for o in walk(self.ask(*args)) if 'id' in o and 'bounds' in o
                and len(o['bounds']) == 4 and o['bounds'][2] > 0 and o['bounds'][3] > 0]

    def find(self, ident, optional=False):
        entries = [e for e in self.entries('path=' + ident) if e['path'].split('.')[-1] == ident]
        if entries:
            return entries[0]
        if not optional:
            raise RuntimeError('visible widget missing: ' + ident)

    def until(self, probe, description):
        while self.c.alive():
            value = probe()
            if value:
                return value
            time.sleep(0.2)
        raise RuntimeError('waiting for ' + description + ': app exited or launch cap reached')

    def widget(self, ident):
        return self.until(lambda: self.find(ident, optional=True), ident)

    def click(self, entry, wait=True):
        return self.input('ui', 'act', 'node=' + entry['id'], 'press', wait=wait)

    def key(self, code, *modifiers):
        return self.input('key', 'k=press', 'c=' + code, *modifiers)

    def text(self, ident):
        return self.find(ident).get('name', '')

    def unchanged_activity(self):
        before = self.seq
        if self.sequence() != before:
            raise RuntimeError('user activity changed during operation window; discard this launch')


def ui_prepare(c, name):
    import sb
    if name not in ('calculator', 'code'):
        raise ValueError('unknown UI workload: ' + name)
    cmd, cwd = sb.start_prepare(c, name)
    if name == 'code':
        cmd += (['--', c.src] if c.kind == 'jit' else [c.src])
    return cmd, cwd


def calculator_setup(r):
    # One repeatable expression; every 40-click window contains five cycles.
    ids = ('k_1', 'k_2', 'k_add', 'k_3', 'k_mul', 'k_4', 'k_eq', 'k_clear')
    buttons = {ident: r.widget(ident) for ident in ids}
    r.c.res['buttons'] = {ident: e['bounds'] for ident, e in buttons.items()}
    return lambda i: r.click(buttons[ids[i % len(ids)]])


def code_setup(r):
    # Open an existing Rust source through the FILES panel's tree filter.
    r.click(r.widget('files_tab_tab'))
    r.click(r.widget('filter'))
    path = 'apps/code/src/ide/editor.rs'
    r.input('text', 't=' + path)
    r.widget('hit_head')
    r.key('ReturnKey')
    r.until(lambda: r.find('status_path', True) and path in r.text('status_path'), 'opened source path')
    # FileEditor exposes its editable surface as CodeView (the inner
    # CodeEditor is not a widget). Its text arrives asynchronously after
    # status_path, so wait for the document before focusing and typing.
    editor = r.until(lambda: next((e for e in r.entries('class=CodeView')
                                  if e['path'].split('.')[-1] == 'code' and e['class'] == 'CodeView' and e.get('name')), None),
                     'loaded CodeView')
    r.click(editor)
    r.key('Home', 'ctrl=1')
    # Prove text input reaches the editor, then restore the buffer. Never save.
    before = r.text('status_caret')
    r.input('text', 't=x')
    after = r.text('status_caret')
    if before == after:
        raise RuntimeError('editor text input did not move the caret')
    r.key('Backspace')
    restored = r.text('status_caret')
    if restored != before:
        raise RuntimeError('editor backspace did not restore the caret')
    r.key('PageDown')
    paged = r.text('status_caret')
    if paged == restored:
        raise RuntimeError('editor PageDown did not move the caret')
    r.key('PageUp')
    if r.text('status_caret') == paged:
        raise RuntimeError('editor PageUp did not move the caret')
    r.c.res.update(source_file=path, editor_caret_probe=[before, after, restored, paged])

    def action(i):
        # Insert/delete and page down/up all change the editor; the buffer stays
        # bounded and returns to its original text after every four actions.
        op = i % 4
        if op == 0:
            r.input('text', 't=x')
        elif op == 1:
            r.key('Backspace')
        elif op == 2:
            r.key('PageDown')
        else:
            r.key('PageUp')
    return action


def ui_drive(c, name):
    import sb
    r = Remote(c)
    first = sb.first_present(c)
    wins = []
    try:
        if first is None:
            raise RuntimeError('no first present')
        r.sequence()
        action = calculator_setup(r) if name == 'calculator' else code_setup(r)
        while c.alive():
            sf = c.settled_from()
            if sf is not None and sum(w['t0'] >= sf for w in wins) >= 3:
                break
            r.sequence()
            t0 = c.now()
            n = int(os.environ.get('SB_UI_OPS', '200'))  # ~6 s per window: >> the 1-s CPU sampling grain
            for i in range(n):
                action(i)
            t1 = c.now()
            r.unchanged_activity()
            wins.append({'t0': t0, 't1': t1, 'ops': n})
        return {'first': first, 'windows': wins, 'unit': 'click' if name == 'calculator' else 'action'}
    finally:
        finish(r)


def stage_prepare(c, name):
    import sb
    if name != 'pdoom':
        raise ValueError('unknown Stage workload: ' + name)
    cmd, cwd = sb.start_prepare(c, 'story')  # Cut (and the P(DOOM) film) live in Makepad Story since the 10-23 split
    if c.kind == 'jit':
        c.env['MAKEPAD_RUN_ZONE_SWAP'] = '0'
    return cmd, cwd


def film_window(r, end, warmup=False):
    t0 = r.c.now()
    tc0 = r.text('cut_time')
    while r.c.now() < end:
        if not r.c.alive():
            raise RuntimeError('app exited during film playback')
        time.sleep(min(0.2, max(0, end - r.c.now())))
    tc1 = r.text('cut_time')
    t1 = r.c.now()
    if tc0 == tc1:
        if warmup:
            return None  # A long settle can outlast the film; rewind below.
        raise RuntimeError('film did not advance: ' + tc0)
    return {'t0': t0, 't1': t1, 'ops': t1 - t0, 'cut_time_t0': tc0, 'cut_time_t1': tc1}


def stage_drive(c, name):
    import sb
    r = Remote(c)
    first = sb.first_present(c)
    wins = []
    try:
        if first is None:
            raise RuntimeError('no first present')
        r.sequence()
        r.input('resize', 'w=1680', 'h=1040')
        cut = r.find('tab_cut', True)
        if not cut:
            raise RuntimeError('Story has no tab_cut')
        r.click(cut)
        card = r.widget('lib_list')
        x, y, _, _ = card['bounds']
        # Concurrent clicks reproduce the library's double-click open gesture.
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            calls = [pool.submit(r.input, 'click', 'x=%g' % (x + 47), 'y=%g' % (y + 56), wait=False)
                     for _ in range(2)]
            for call in calls:
                call.result()
        r.until(lambda: (e := r.find('cut_play', True)) and e['bounds'][1] < 700, 'opened film transport')
        dismiss = r.find('cut_song_dismiss', True)
        if dismiss:
            r.click(dismiss)
        tc = r.text('cut_time')
        r.click(r.widget('cut_play'))
        r.until(lambda: r.text('cut_time') != tc, 'moving film timecode')
        c.res['playback_first'] = c.now()
        # play (10-s windows, for time-to-final-perf) until the settle time and the JIT's idle; a film that ends
        # meanwhile just yields no window
        while c.alive() and (c.settled_from() is None or c.now() < c.settled_from()):
            window = film_window(r, c.now() + 10, warmup=True)
            if window:
                wins.append(window)
        # Pause first so every settled run starts at zero, even if warmup
        # outlasted the 2:36 film. Seeking alone may preserve playback state.
        pause = r.find('cut_pause', True)
        if pause:
            r.click(pause)
        r.click(r.widget('cut_to_start'))
        tc = r.text('cut_time')
        if any(ch not in '0:' for ch in tc) or not tc:
            raise RuntimeError('seek did not reach timecode zero: ' + tc)
        play = r.find('cut_play', True)
        if play:
            r.click(play)
        start = c.now()
        c.res['segment_start'] = start
        for i in range(6):
            w = film_window(r, start + (i + 1) * 10)
            w['seg'] = True  # the measured segment: the same film content on every launch
            wins.append(w)
        r.unchanged_activity()
        return {'first': first, 'windows': wins, 'unit': 'film-s'}
    finally:
        finish(r)


def finish(r):
    """Graceful shutdown; sb.py retains its owned process-group fallback."""
    if r.c.p.poll() is None and r.c.now() < r.c.cap:
        try:
            r.sequence()
            r.input('quit', wait=False)
            r.c.p.wait(timeout=10)
        except Exception:
            pass


ADAPTERS = {'ui': (ui_prepare, ui_drive), 'stage': (stage_prepare, stage_drive)}
