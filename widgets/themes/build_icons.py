"""Build the editable SVG application-icon families; stdlib only.

Run from any directory with `python3 widgets/themes/build_icons.py`.
The resulting SVGs are embedded by app_icon.rs and hotload from a checkout.
"""
from pathlib import Path
import math

ROOT = Path(__file__).parent
COLORS = {
    'applications': '#737eae', 'browser': '#168df5', 'files': '#00b9f1',
    'terminal': '#252b34', 'mixer': '#f05a83', 'task': '#35bb86',
    'sheets': '#24a366', 'photos': '#f083b4', 'fabric': '#539ade',
    'score': '#ef7b39', 'video': '#8c5cdd', 'route': '#65b876',
    'vj': '#c159d5', 'fab': '#e5a044', 'studio': '#526fdf',
    'image': '#699fe3', 'pdf': '#e9515a', 'aichat': '#28a899',
    'counter': '#f2725c', 'app': '#7589bf',
}

def path(d, fill='none', stroke='#ffffff', width=3):
    return f'<path d="{d}" fill="{fill}" stroke="{stroke}" stroke-width="{width}" stroke-linecap="round" stroke-linejoin="round"/>'

def rect(x,y,w,h,fill,rx=0,stroke='none',sw=1):
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" fill="{fill}" stroke="{stroke}" stroke-width="{sw}"/>'

def circle(x,y,r,fill,stroke='none',sw=1):
    return f'<circle cx="{x}" cy="{y}" r="{r}" fill="{fill}" stroke="{stroke}" stroke-width="{sw}"/>'

def tint(color, amount):
    channels = [int(color[i:i+2], 16) for i in (1, 3, 5)]
    target = 255 if amount > 0 else 0
    return '#' + ''.join(f'{round(c + (target-c)*abs(amount)):02x}' for c in channels)

def mac_tile(fill):
    # Soft contact shadow and an inset highlight, both intentionally restrained
    # so the dock's glass supplies the surrounding depth.
    shadow = '<g opacity="0.12">' + rect(3,4.5,58,57,'#101521',13) + '</g>'
    return shadow + rect(3,3,58,57,fill,13)

def mac_glint(color='#ffffff'):
    return rect(3.5,3.5,57,56,'url(#shine)',12.5) + '<g opacity="0.20">' + rect(3.6,3.6,56.8,55.8,'none',12.4,color,.55) + '</g>'

def symbol(name):
    white='#ffffff'
    if name == 'files':
        return path('M12 23 V17 H28 L33 23 H52 V47 H12 Z', '#fff5c4', '#ffe69a',1.5)+path('M12 26 H53 L49 47 H10 Z','#ffce65','#fff0ad',1.5)
    if name == 'browser':
        # An original web-window identity: a globe inside a browser frame.
        return (rect(8,12,48,42,'#edf8ff',6)
                +path('M8 23 V18 Q8 12 14 12 H50 Q56 12 56 18 V23 Z','#bddbf3','none')
                +circle(14,18,1.4,'#648cad')+circle(19,18,1.4,'#648cad')
                +rect(25,16,24,4,'#f8fcff',2)
                +circle(32,37,12,'url(#ocean)')
                +path('M20 37 H44 M32 25 C23 31 23 43 32 49 M32 25 C41 31 41 43 32 49',stroke='#c7f4ff',width=1.3)
                +path('M22 31 Q32 35 42 31 M22 43 Q32 39 42 43',stroke='#c7f4ff',width=1.1))
    if name == 'terminal':
        return rect(9,12,46,39,'#111820',6,'#7d8590',1)+path('M17 23 L25 30 L17 37 M31 38 H44',width=2.5)
    if name == 'applications':
        cols=['#43ccff','#ff647f','#ffb84d','#65d483','#ab8bef','#59d7d7','#ffcf5f','#62a9ff','#ee7bb8']
        return ''.join(rect(13+(i%3)*14,13+(i//3)*14,10,10,c,2.5) for i,c in enumerate(cols))
    if name == 'mixer':
        return path('M18 16 V49 M32 16 V49 M46 16 V49',stroke='#ffbfce',width=2)+rect(13,25,10,7,white,2)+rect(27,37,10,7,white,2)+rect(41,20,10,7,white,2)
    if name == 'task':
        return rect(9,12,46,39,'#112c29',5,'#7be6ba',1)+path('M14 40 H23 L28 22 L33 43 L40 29 L44 35 H50',stroke='#77ffd0',width=2.4)
    if name == 'sheets':
        return rect(14,9,38,47,'#f6fff8',3)+rect(14,9,38,12,'#b8eaca',3)+''.join(rect(20+c*9,27+r*8,6,5,'#3da777' if c==0 else '#c7ead6',1) for r in range(3) for c in range(3))+rect(8,25,16,20,'#17804e',2)+path('M12 30 H20 M12 35 H20 M12 40 H20',width=1.5)
    if name == 'photos':
        petals=['#ff5870','#ff973d','#ffd746','#83d252','#36c8ac','#44aaf7','#8279ed','#d768da']
        return ''.join(f'<ellipse cx="32" cy="21" rx="8" ry="15" fill="{c}" opacity="0.9" transform="rotate({i*45} 32 32)"/>' for i,c in enumerate(petals))+circle(32,32,7,'#fff3c3')
    if name == 'fabric':
        return path('M22 14 L14 18 L8 30 L19 34 L22 28 V51 H43 V28 L46 34 L56 30 L50 18 L42 14 Q32 24 22 14 Z','#f6fcff','#d8ebff',1)+path('M26 17 Q32 25 38 17 M25 47 H39',stroke='#83b8ee',width=2)
    if name == 'score':
        return path('M28 42 V19 L47 15 V38',width=4)+path('M28 22 L47 18',width=4)+f'<ellipse cx="22" cy="43" rx="7" ry="5" fill="white"/><ellipse cx="41" cy="39" rx="7" ry="5" fill="white"/>'
    if name == 'video':
        return rect(9,16,46,35,'#f5f0ff',5)+path('M27 25 L41 34 L27 43 Z','#8650d2','none')+path('M10 16 H54 V24 H10 Z','#252432','none')+path('M18 16 L14 24 M31 16 L27 24 M44 16 L40 24',stroke='#e7dff7',width=5)
    if name == 'route':
        return path('M9 14 L24 10 L41 15 L55 10 V49 L41 54 L24 49 L9 54 Z','#e1f2dc','#edf9f0',1)+path('M24 11 V49 M41 16 V53 M10 38 L53 25',stroke='#fff',width=5)+path('M13 38 L25 34 L31 23 L48 19',stroke='#f3c969',width=3)+path('M43 15 C32 15 32 28 43 36 C54 28 54 15 43 15 Z','#fa5961','none')+circle(43,22,3,'white')
    if name == 'vj':
        return circle(32,32,23,'#242334','#c8bce6',1)+circle(32,32,17,'none','#5b526f',1)+circle(32,32,12,'none','#5b526f',1)+circle(32,32,7,'#e880d0')+circle(32,32,2,'white')+path('M49 13 L52 17 L43 38 L38 42',stroke='#dce4f3',width=3)
    if name == 'fab':
        return path('M32 10 L52 21 V43 L32 55 L12 43 V21 Z','#fff4d4','#ffffff',1)+path('M32 10 V32 M12 21 L32 32 L52 21 M32 32 V55',stroke='#d18c3a',width=2.5)
    if name == 'studio':
        return rect(8,12,48,40,'#172842',5,'#a4c3ff',1)+path('M25 24 L17 32 L25 40 M39 24 L47 32 L39 40 M35 21 L29 43',stroke='#80d1ff',width=2.8)+circle(15,18,1.7,'#ff8188')+circle(21,18,1.7,'#ffd578')+circle(27,18,1.7,'#77d49e')
    if name == 'image':
        return rect(9,12,46,40,white,4)+rect(13,16,38,30,'url(#ocean)',2)+circle(42,24,5,'#fff0a0')+path('M13 43 L25 28 L36 40 L42 33 L51 43 V46 H13 Z','#c7efcb','none')
    if name == 'pdf':
        return path('M16 8 H40 L50 19 V56 H16 Z','#fff8f4','none')+path('M40 8 V19 H50',stroke='#f2b8b8',width=1.5)+path('M24 43 Q37 14 34 25 Q35 39 44 41 Q31 34 22 45',stroke='#d7444e',width=2.3)+rect(12,15,23,9,'#c93e4c',1)+path('M16 18 V21 M21 18 V21 M26 18 V21 M30 18 V21',width=1.2)
    if name == 'aichat':
        return path('M12 16 Q12 12 18 12 H47 Q53 12 53 18 V39 Q53 44 47 44 H30 L19 53 V44 H18 Q12 44 12 38 Z',white,'none')+circle(23,28,2.4,'#289c8d')+circle(32,28,2.4,'#289c8d')+circle(41,28,2.4,'#289c8d')
    if name == 'counter':
        return rect(12,29,9,22,'#ffdab5',2)+rect(27,20,9,31,'white',2)+rect(42,11,9,40,'#ffe8c8',2)
    return rect(11,11,42,42,white,7)+rect(15,17,34,8,'#cadaef',2)+rect(15,29,14,19,'#92add1',2)+path('M34 32 H46 M34 39 H46 M34 46 H43',stroke='#91a6c7',width=2)

def pixel_icon(name):
    """Deliberately hand-grid 16px artwork with the classic 16-color palette."""
    grid=[['' for _ in range(16)] for _ in range(16)]
    def box(x,y,w,h,c):
        for yy in range(max(0,y),min(16,y+h)):
            for xx in range(max(0,x),min(16,x+w)): grid[yy][xx]=c
    def dot(x,y,c): box(x,y,1,1,c)
    navy='#000080'; black='#000000'; gray='#808080'; light='#c0c0c0'; white='#ffffff'; teal='#008080'; yellow='#ffff00'
    if name=='files':
        box(1,4,13,10,black);box(2,2,5,3,black);box(3,3,3,2,yellow)
        box(2,5,11,8,'#808000');box(3,6,12,8,black)
        box(4,7,10,6,yellow);box(4,7,9,1,white)
    elif name=='browser':
        for y in range(1,15):
            for x in range(1,15):
                d=(x-7.5)**2+(y-7.5)**2
                if d<47: dot(x,y,navy if d>32 else '#0000ff')
        for x,y in [(5,3),(6,3),(4,4),(5,4),(3,5),(4,5),(5,5),(5,6),(6,7),(7,8),(8,9),(8,10),(9,11),(9,12),(10,5),(11,5),(10,6)]: dot(x,y,'#00ff00')
        box(5,2,4,1,'#00ffff')
    elif name=='route':
        box(1,3,5,11,black);box(6,1,5,11,black);box(11,3,4,11,black)
        box(2,4,4,9,'#008000');box(6,2,5,9,'#00ff00');box(11,4,3,9,'#008000')
        box(3,8,9,2,yellow);box(7,3,2,9,white);box(10,5,3,3,'#ff0000');dot(11,8,'#ff0000')
    elif name=='photos':
        box(1,5,14,9,black);box(2,6,12,7,light);box(3,3,5,3,black);box(4,4,3,2,gray)
        box(5,7,7,5,gray);box(6,6,5,7,gray);box(6,8,5,3,black);box(7,7,3,5,black)
        box(8,8,2,2,'#000080');dot(8,8,'#00ffff');box(2,6,2,2,white);box(12,6,2,1,white)
    elif name=='vj':
        for y in range(1,15):
            for x in range(1,15):
                d=(x-7.5)**2+(y-7.5)**2
                if d<47: dot(x,y,black if d>36 else light)
        box(4,4,3,2,white);box(3,6,2,3,'#00ffff');box(10,9,3,2,'#ff00ff');box(8,11,3,2,'#0000ff')
        box(6,6,4,4,gray);box(7,7,2,2,black)
    elif name in ('terminal','task','studio','app','vj','mixer'):
        box(1,1,14,11,black);box(2,2,12,9,light);box(3,3,10,7,navy if name=='terminal' else black);box(5,12,6,1,gray);box(4,13,8,2,light);box(3,15,10,1,black)
        if name=='terminal':
            dot(4,5,white);dot(5,6,white);dot(4,7,white);box(7,7,3,1,white)
        elif name=='studio':
            box(4,4,7,1,'#00ffff');box(4,6,5,1,white);box(4,8,8,1,'#ffff00')
        else:
            for i,h in enumerate([2,5,3,6]): box(4+i*2,10-h,1,h,'#00ff00' if name=='task' else ['#ff0000',yellow,'#00ffff','#ff00ff'][i])
    elif name=='applications':
        for i,c in enumerate(['#ff0000','#00ff00','#0000ff',yellow]):
            x=1+(i%2)*7;y=1+(i//2)*7;box(x+1,y+1,6,6,black);box(x,y,6,6,c);box(x,y,6,1,white);box(x,y,1,6,white)
    else:
        box(3,0,8,1,black);box(2,1,12,15,black);box(3,1,7,13,white);box(10,4,3,10,white);box(10,1,1,3,gray);box(11,2,1,2,gray);box(12,3,1,1,gray)
        if name=='sheets':
            for y in [5,8,11]: box(4,y,8,1,teal)
            for x in [4,8,11]: box(x,5,1,7,teal)
        elif name in ('photos','image','fabric'):
            box(4,5,8,7,'#00ffff');box(5,9,6,3,'#008000');box(7,7,3,5,'#008000');box(9,6,2,2,yellow)
            if name=='fabric': box(6,7,4,6,'#ff00ff')
        elif name in ('score','video'):
            box(8,5,1,7,navy);box(8,5,4,1,navy);box(11,5,1,5,navy);box(5,11,3,2,navy);box(9,9,3,2,navy)
            if name=='video': box(4,5,2,8,black)
        elif name=='pdf':
            box(3,5,10,3,'#ff0000');box(5,9,6,1,gray);box(5,11,5,1,gray)
        elif name=='aichat':
            box(4,5,8,6,navy);box(5,6,6,4,white);dot(6,8,black);dot(9,8,black);dot(5,11,navy)
        elif name=='counter':
            box(4,10,2,3,'#ff0000');box(7,8,2,5,'#008000');box(10,6,2,7,'#0000ff')
        else:
            box(5,6,6,6,'#808000');box(6,5,5,1,yellow);box(5,6,1,5,yellow);box(11,7,1,5,gray)
    body=''
    for y,row in enumerate(grid):
        x=0
        while x<16:
            c=row[x]; end=x+1
            while end<16 and row[end]==c: end+=1
            if c: body+=rect(x,y,end-x,1,c)
            x=end
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">{body}</svg>\n'

def icon(name, style):
    if style=='windows-2000': return pixel_icon(name)
    color=COLORS[name]
    if style in ('macos','ios'):
        color = {'applications':'#555fc9','browser':'#087dff','files':'#009fff',
                 'mixer':'#ff285b','task':'#00a66a','sheets':'#009649','fabric':'#147dff',
                 'score':'#ff6417','video':'#7130e8','route':'#28a647','vj':'#af22d1',
                 'fab':'#e98906','studio':'#175ad9','image':'#227fff','pdf':'#ec3039',
                 'aichat':'#009c88','counter':'#ff573b'}.get(name,color)
    defs=f'<defs><linearGradient id="tile" x2="0" y2="1"><stop stop-color="{tint(color,.04) if style in ("macos","ios") else tint(color,.16)}"/><stop offset="1" stop-color="{tint(color,-.18)}"/></linearGradient><linearGradient id="ocean" x2="0" y2="1"><stop stop-color="#20c9f5"/><stop offset="1" stop-color="#096ade"/></linearGradient><linearGradient id="shine" x2="0" y2="1"><stop stop-color="#ffffff" stop-opacity="0.12"/><stop offset="0.65" stop-color="#ffffff" stop-opacity="0"/></linearGradient></defs>'
    art=symbol(name)
    if style=='macos':
        tile='#f7f8fa' if name in ('photos','route') else 'url(#tile)'
        body=mac_tile(tile)+art+mac_glint()
        if name=='files':
            # Layered documents in a folder, with no platform mascot or face.
            body=(mac_tile('#f5eee3')
                  +path('M10 22 V18 Q10 15 14 15 H26 L32 21 H50 Q54 21 54 25 V47 H10 Z','#71c7e5','none')
                  +rect(20,18,25,29,'#d7e3ee',3)
                  +rect(17,21,28,27,'#ffffff',3)
                  +path('M22 26 H38 M22 30 H34',stroke='#aec6d9',width=1.7)
                  +path('M10 31 Q10 29 13 29 H52 Q55 29 54 32 L50 48 Q49 51 46 51 H13 Q10 51 10 48 Z','url(#ocean)','none')
                  +path('M14 32 H50',stroke='#b6efff',width=1.2)
                  +rect(17,40,13,3,'#c2f3ff',1.5)+mac_glint())
    elif style=='nextstep':
        body=(rect(0,0,64,64,'#000000')+rect(1,1,62,62,'#aaaaaa')
              +path('M1 62 V1 H62',stroke='#ffffff',width=2)
              +path('M2 62 H62 V2',stroke='#555555',width=2)+art)
    elif style=='ios':
        # Current iOS: full-bleed continuous corners, layered color and a
        # restrained glass rim. Original symbols, never platform app mascots.
        tile = '#f9fafc' if name in ('photos','route','files') else 'url(#tile)'
        if name=='files':
            art=path('M10 23 V17 H28 L33 23 H54 V49 H10 Z','#ffda66','none')+path('M10 27 H55 L50 49 H9 Z','#ffba08','none')
        outline='M17 1 H47 C59 1 63 5 63 17 V47 C63 59 59 63 47 63 H17 C5 63 1 59 1 47 V17 C1 5 5 1 17 1 Z'
        body=(path(outline,tile,'none')+art
              +path(outline,'none',tint(color,.30),.35))
    elif style=='android':
        # Pixel's adaptive circle mask with saturated foreground layers.
        # Keep all artwork inside the mask's safe zone.
        body=(circle(32,32,31,tint(color,.80))
              +f'<g transform="translate(7 7) scale(0.78)">{art}</g>')
    elif style=='windows':
        # Fluent-like silhouette and restrained two-tone color; no macOS tile.
        if name=='applications':
            body=''.join(rect(10+x*24,10+y*24,20,20,'#168df5') for x in range(2) for y in range(2))
        elif name=='browser':
            body=circle(32,32,24,'url(#ocean)')+path('M11 39 C9 14 34 8 46 22 C32 12 13 23 22 38 C29 48 44 44 53 36 C49 60 16 62 11 39 Z','#29c6a4','none')+path('M17 35 C24 24 40 24 51 32',stroke='#b4fff0',width=3)
        elif name in ('route','sheets','photos','image','pdf','video','files','fab'): body=art
        else: body=rect(6,6,52,52,color,6)+art
    else:
        # Omarchy uses one ink color; the renderer can tint this semantic icon.
        glyph={
            'files':'M10 20 H27 L32 26 H54 V51 H10 Z M10 20 V14 H27 L32 20 H50 V26',
            'browser':'M32 10 A22 22 0 1 0 32 54 A22 22 0 1 0 32 10 M10 32 H54 M32 10 C19 22 19 42 32 54 M32 10 C45 22 45 42 32 54',
            'terminal':'M10 14 H54 V50 H10 Z M18 24 L26 31 L18 38 M33 39 H44',
            'applications':'M12 12 H26 V26 H12 Z M38 12 H52 V26 H38 Z M12 38 H26 V52 H12 Z M38 38 H52 V52 H38 Z',
            'mixer':'M17 12 V52 M32 12 V52 M47 12 V52 M11 23 H23 M26 42 H38 M41 31 H53',
            'task':'M9 40 H20 L27 14 L35 50 L43 25 L49 36 H56',
            'score':'M27 44 V16 L49 12 V39 M27 21 L49 17 M27 44 C12 35 11 52 23 49 Z M49 39 C34 30 33 47 45 44 Z',
            'sheets':'M15 8 H49 V56 H15 Z M22 22 H43 M22 31 H43 M22 40 H43 M30 22 V48 M39 22 V48',
            'photos':'M32 10 C49 -2 58 23 43 26 C65 34 45 55 36 41 C30 63 9 46 23 36 C1 36 12 11 26 23 Z',
            'video':'M9 15 H55 V50 H9 Z M26 25 L42 33 L26 42 Z',
            'route':'M9 16 L24 10 L42 16 L55 10 V49 L42 55 L24 49 L9 55 Z M24 10 V49 M42 16 V55',
            'studio':'M23 21 L12 32 L23 43 M41 21 L52 32 L41 43 M37 13 L27 51',
            'fabric':'M22 13 L12 18 L6 30 L18 35 L21 28 V53 H43 V28 L46 35 L58 30 L52 18 L42 13 Q32 24 22 13 Z',
            'vj':'M32 9 A23 23 0 1 0 32 55 A23 23 0 1 0 32 9 M32 25 A7 7 0 1 0 32 39 A7 7 0 1 0 32 25 M51 13 L55 19 L44 43',
            'fab':'M32 9 L53 21 V44 L32 56 L11 44 V21 Z M11 21 L32 33 L53 21 M32 33 V56',
            'aichat':'M10 13 H54 V43 H29 L18 54 V43 H10 Z M20 26 H44 M20 34 H38',
            'counter':'M13 33 H22 V52 H13 Z M28 23 H37 V52 H28 Z M43 12 H52 V52 H43 Z',
            'image':'M9 12 H55 V52 H9 Z M10 46 L25 28 L38 41 L45 32 L55 44 M43 19 A4 4 0 1 0 43 27 A4 4 0 1 0 43 19',
            'pdf':'M16 8 H39 L50 19 V56 H16 Z M39 8 V19 H50 M24 43 Q37 14 34 25 Q35 39 44 41 Q31 34 22 45',
        }.get(name,'M11 11 H53 V53 H11 Z M11 24 H53 M25 24 V53')
        body=path(glyph,stroke='#d5dce8',width=2.6)
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64">{defs}{body}</svg>\n'

FILE_KINDS = {'folder':'files','generic':'app','image':'image','text':'app','code':'studio','audio':'score','video':'video','archive':'fab','pdf':'pdf'}

def file_icon(kind, style):
    """Document identities use the same hot-loaded catalog as application icons."""
    app = FILE_KINDS[kind]
    if style == 'windows-2000':
        return pixel_icon(app)
    if style == 'omarchy':
        return (ROOT.parent.parent / 'apps/files/resources/icons' / (('file' if kind == 'generic' else kind) + '.svg')).read_text()
    if kind == 'folder':
        back, front = ('#81d4ff','#2199ed') if style in ('macos','ios') else ('#ffe39b','#fabb32')
        body = path('M5 18 V12 H27 L34 18 H58 V52 H5 Z',back,'none') + path('M5 24 H59 L54 52 H5 Z',front,'none')
    else:
        body = symbol(app)
    document = icon(app,style)
    defs = document[document.index('<defs>'):document.index('</defs>')+7]
    return '<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64">'+defs+body+'</svg>\n'

if __name__=='__main__':
    for style in ['omarchy','macos','windows','windows-2000','nextstep','ios','android']:
        out=ROOT/style/'icons';out.mkdir(parents=True,exist_ok=True)
        for name in COLORS: (out/f'{name}.svg').write_text(icon(name,style))
        for kind in FILE_KINDS: (out/f'file-{kind}.svg').write_text(file_icon(kind,style))
