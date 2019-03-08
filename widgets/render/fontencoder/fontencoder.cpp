#include "msdfgen.h"
#include "msdfgen-ext.h"

using namespace msdfgen;

int main(int argc, const char **argv) {


    if(argc < 6){
        printf("fontencoder font.ttf out.font <scale 2.5> <start unicode> <end unicode> <unicodeset> out.font\n");
        return -1;
    }

    const char *fontFile = argv[1];
    const char *outFile = argv[2];

    FreetypeHandle *ft = initializeFreetype();
    if (!ft) {
        printf("Cannot initialize FreeType\n");
        return -1;
    }

    FontHandle *font = loadFont(ft, fontFile);
    if (!font) {
        printf("Cannot load font %s\n", fontFile);
        deinitializeFreetype(ft);
        return -1;
    }

    // output buffer

    double scale = strtod(argv[3], NULL);

    int outWidth = 4096;
    int outHeight = 4096;
    //int len = end-start;
    //var 

    int maxInput = 65536;
    unsigned char *inputSet = new unsigned char[maxInput];
    memset(inputSet, maxInput, 0);

    if(argc == 6){
        int start = strtol(argv[4], NULL,16);
        int end = strtol(argv[5], NULL,16);
        for(int c = start;c<end;c++){
            inputSet[c] = 1;
        }
    }
    else{
        for(int c = 4; c < argc; c++){
            int item = strtol(argv[c], NULL,16);
            inputSet[item] = 1;
        }
    }

    //int outputLen = outWidth*outHeight*3;
    //unsigned char *output = new unsigned char[outputLen];

    int outputLen = outWidth*outHeight;
    unsigned char *output1 = new unsigned char[outputLen];
    unsigned char *output2 = new unsigned char[outputLen];
    unsigned char *output3 = new unsigned char[outputLen];
    unsigned char *output4 = new unsigned char[outputLen];
    memset(output1, outputLen, 0);    
    memset(output2, outputLen, 0);    
    memset(output3, outputLen, 0);    
    memset(output4, outputLen, 0);    
   // memset(output, outputLen, 0);    

    float* glyph_x1 = new float[maxInput];
    float* glyph_y1 = new float[maxInput];
    float* glyph_x2 = new float[maxInput];
    float* glyph_y2 = new float[maxInput];
    float* glyph_advance = new float[maxInput];
    int* glyph_unicode = new int[maxInput];
    
    int* glyph_toffset = new int[maxInput];
    int* glyph_tsingle = new int[maxInput];
    int* glyph_tw = new int[maxInput];
    int* glyph_th = new int[maxInput];

    int bitmapW = ceil(64 * scale);
    int bitmapH = ceil(64 * scale);
    int padX = 1;
    int padY = 1;
    int moveX = 8.;
    int moveY = 8;
    int ox = 0, oy = 0, mh = 0;

    double fontScale = 0;
    getFontScale(fontScale, font);

    int o1 = 0;
    int o2 = 0;
    int slot = 0;
    for(int i = 0; i < maxInput; i++){
        Shape shape;
        double advance = 0.;
        if(inputSet[i] == 0 || !getCharIndex(font, i)){
            continue;
        }
        if (loadGlyph(shape, font, i, &advance)) {
            shape.normalize();
            
            double l = 100000, b = 100000, r = -100000, t = -100000;

            shape.bounds(l, b, r, t);

            double x1 = l * scale+moveX* scale;
            double y1 = t * scale+moveY* scale;
            double x2 = r * scale+moveX* scale;
            double y2 = b * scale+moveY* scale;

            // snap to pixel boundary with 2 pixel safety
            int ix1 = floor(x1) - 2;
            int iy1 = ceil(y1) + 2;
            int ix2 = ceil(x2) + 2;
            int iy2 = floor(y2) - 2;

            if(l == 100000 || advance == 0.){
                continue;
            }

            glyph_unicode[slot] = i;

            glyph_advance[slot] = (float)advance/fontScale;

            // max. angle
            edgeColoringSimple(shape, 3.0);
            
            // image width, height
            Bitmap<FloatRGB> msdf(bitmapW, bitmapH);
            generateMSDF(msdf, shape, 4.0, scale, Vector2(moveX,moveY));// Vector2(4.0, 4.0));
 
            // the texture error to move into the geometry
            float ex1 = (2. + (x1 - floor(x1)))/scale;
            float ey1 = (2. + (ceil(y1) - y1))/scale;
            float ex2 = (2. + (ceil(x2) - x2))/scale;
            float ey2 = (2. + (y2 - floor(y2)))/scale;

            glyph_x1[slot] = (l-ex1)/fontScale;
            glyph_y1[slot] = (t+ey1)/fontScale;
            glyph_x2[slot] = (r+ex2)/fontScale;
            glyph_y2[slot] = (b-ey2)/fontScale;
            // unscaled font coordinates
            
            // compute actual w/h
            int th = iy1 - iy2;
            int tw = ix2 - ix1;

            glyph_tw[slot] = tw;
            glyph_th[slot] = th;

            if(th > mh) mh = th;
            // first figure out if MSDF can be encoded in 1 channel
            int isSingle = true;
            for(int y = 0; y < th; y++){
                for(int x = 0; x < tw; x++){
                    int bx = x + ix1;
                    int by = y + iy2;
                    int v1 = clamp(int(msdf(bx, by).r*0x100), 0xff);
                    int v2 = clamp(int(msdf(bx, by).g*0x100), 0xff);
                    int v3 = clamp(int(msdf(bx, by).b*0x100), 0xff);
                    if(v1 != v2 || v2 != v3 || v1 != v3){
                        isSingle = false;
                    } 
                }
            }
            glyph_tsingle[slot] = isSingle;
            if(isSingle){
                glyph_toffset[slot] = o2;
                for(int y = 0; y < th; y++){
                    for(int x = 0; x < tw; x++){
                        int bx = x + ix1;
                        int by = y + iy2;
                        output4[o2++] = clamp(int(msdf(bx, by).r*0x100), 0xff);
                    }
                }
            }
            else{
                glyph_toffset[slot] = o1;
                 for(int y = 0; y < th; y++){
                    for(int x = 0; x < tw; x++){
                        int bx = x + ix1;
                        int by = y + iy2;
                        output1[o1] = clamp(int(msdf(bx, by).r*0x100), 0xff);
                        output2[o1] = clamp(int(msdf(bx, by).g*0x100), 0xff);
                        output3[o1] = clamp(int(msdf(bx, by).b*0x100), 0xff);
                        o1++;
                    }
                }
            }

            printf("unicode: %d w: %d h: %d single: %d x: %d y: %d\n", i, tw, th, isSingle, ox, oy);

            ox += tw+padX;
            if(ox+tw >= outWidth) ox = 0, oy += mh+padY, mh = 0;

            slot++;
        }
    }
    
    int finalHeight = 1;

    while(finalHeight < oy + mh){
        finalHeight *= 2;
    }

    int headerLen = (slot)*10*4+4*6;
    char* header = new char[headerLen];
    unsigned short *u16 = (unsigned short*)header;
    unsigned int *u32 = (unsigned int*)header;
    float *f32 = (float*)header;

    char *kerntable = new char[slot*slot*4];
    float *kernf32 = (float*)kerntable;
    unsigned int *kernu32 = (unsigned int*) kerntable;
    int kernsize = 0;
    for(int i = 0; i < slot; i++){
        for(int j = 0; j < slot; j++){
            double kern = 0;
            if(getKerning(kern, font, glyph_unicode[i], glyph_unicode[j]) && kern != 0.){
                kernu32[kernsize++] = i;
                kernu32[kernsize++] = j;
                kernf32[kernsize++] = kern / fontScale;
            }
        }
    }

    // output header
    u32[0] = 0x03F01175;
    u16[2] = outWidth;
    u16[3] = finalHeight;
    u32[2] = slot;
    u32[3] = o1;
    u32[4] = o2;
    u32[5] = kernsize / 3;
    // write out font table
    for(int i = 0; i < slot; i++){
        int o = i*10 + 6;
        u32[o+0] = glyph_unicode[i];
        f32[o+1] = glyph_x1[i];
        f32[o+2] = glyph_y1[i];
        f32[o+3] = glyph_x2[i];
        f32[o+4] = glyph_y2[i];
        f32[o+5] = glyph_advance[i];
        u32[o+6] = glyph_tsingle[i];
        u32[o+7] = glyph_toffset[i];
        u32[o+8] = glyph_tw[i];
        u32[o+9] = glyph_th[i];
    }
  
    // lets fwrite our font.
    FILE *f = fopen(outFile, "wb");
    if(!f){
        printf("Cannot open output %s\n", outFile);
        deinitializeFreetype(ft);
        return -1;
    }

    fwrite(header, headerLen, 1., f);
    fwrite(kerntable, kernsize*4, 1., f);
    fwrite(output1, o1, 1., f);
    fwrite(output2, o1, 1., f);
    fwrite(output3, o1, 1., f);
    fwrite(output4, o2, 1., f);
    // write end padding
    int endBytes = headerLen + 3*o1 + o2;
    int padBytes = 0;
    fwrite(&padBytes, 4-(endBytes&3), 1., f);
    
    fclose(f);

    destroyFont(font);
    
    deinitializeFreetype(ft);
    
    printf("Written %dx%d %d glyphs as %s\n", outWidth, finalHeight, slot, outFile);

    return 0;
}