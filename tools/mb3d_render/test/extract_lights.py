import struct
with open('../../local/mb3d/cathedral.m3p', 'rb') as f:
    data = f.read()
    offset = 508
    print("Lights:")
    for i in range(3):
        l_option = data[offset]
        l_function = data[offset+1]
        l_color = list(data[offset+2:offset+5])
        print(f"  {i}: opt={l_option}, func={l_function}, col={l_color}")
        offset += 32
