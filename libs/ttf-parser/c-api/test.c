#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "ttfparser.h"

void move_to_cb(float x, float y, void *data)
{
    uint32_t *counter = (uint32_t*)data;
    *counter += 1;
}

void line_to_cb(float x, float y, void *data)
{
    uint32_t *counter = (uint32_t*)data;
    *counter += 1;
}

void quad_to_cb(float x1, float y1, float x, float y, void *data)
{
    uint32_t *counter = (uint32_t*)data;
    *counter += 1;
}

void curve_to_cb(float x1, float y1, float x2, float y2, float x, float y, void *data)
{
    uint32_t *counter = (uint32_t*)data;
    *counter += 1;
}

void close_path_cb(void *data)
{
    uint32_t *counter = (uint32_t*)data;
    *counter += 1;
}

int main() {
    // Read the file first.
    FILE *file = fopen("../benches/fonts/SourceSansPro-Regular.ttf", "rb");
    if (file == NULL) {
        return -1;
    }

    fseek(file, 0, SEEK_END);
    long fsize = ftell(file);
    fseek(file, 0, SEEK_SET);

    char *font_data = (char*)malloc(fsize + 1);
    fread(font_data, 1, fsize, file);
    fclose(file);

    // Test functions.
    // We mainly interested in linking errors.
    assert(ttfp_fonts_in_collection(font_data, fsize) == -1);

    ttfp_face *face = (ttfp_face*)alloca(ttfp_face_size_of());
    assert(ttfp_face_init(font_data, fsize, 0, face));

    uint16_t a_gid = ttfp_get_glyph_index(face, 0x0041); // A
    assert(a_gid == 2);
    assert(ttfp_get_glyph_index(face, 0xFFFFFFFF) == 0);
    assert(ttfp_get_glyph_var_index(face, 0x0041, 0xFE03) == 0);

    assert(ttfp_get_glyph_hor_advance(face, 0x0041) == 544);
    assert(ttfp_get_glyph_hor_side_bearing(face, 0x0041) == 3);
    assert(ttfp_get_glyph_ver_advance(face, 0x0041) == 0);
    assert(ttfp_get_glyph_ver_side_bearing(face, 0x0041) == 0);
    assert(ttfp_get_glyph_y_origin(face, a_gid) == 0);

    assert(ttfp_get_name_records_count(face) == 20);
    ttfp_name_record record;
    assert(ttfp_get_name_record(face, 100, &record) == false);
    assert(ttfp_get_name_record(face, 1, &record) == true);
    assert(record.name_id == 1);

    char family_name[30];
    assert(ttfp_get_name_record_string(face, 1, family_name, 30));

    assert(ttfp_get_units_per_em(face) == 1000);
    assert(ttfp_get_ascender(face) == 984);
    assert(ttfp_get_descender(face) == -273);
    assert(ttfp_get_height(face) == 1257);
    assert(ttfp_get_line_gap(face) == 0);
    assert(ttfp_is_regular(face) == true);
    assert(ttfp_is_italic(face) == false);
    assert(ttfp_is_bold(face) == false);
    assert(ttfp_is_oblique(face) == false);
    assert(ttfp_get_weight(face) == 400);
    assert(ttfp_get_width(face) == 5);
    assert(ttfp_get_x_height(face) == 486);
    assert(ttfp_get_number_of_glyphs(face) == 1974);

    ttfp_rect g_bbox = ttfp_get_global_bounding_box(face);
    assert(g_bbox.x_min == -454);
    assert(g_bbox.y_min == -293);
    assert(g_bbox.x_max == 2159);
    assert(g_bbox.y_max == 968);

    ttfp_line_metrics line_metrics;
    assert(ttfp_get_underline_metrics(face, &line_metrics));
    assert(line_metrics.position == -50);
    assert(line_metrics.thickness == 50);

    assert(ttfp_get_strikeout_metrics(face, &line_metrics));
    assert(line_metrics.position == 291);
    assert(line_metrics.thickness == 50);

    ttfp_script_metrics script_metrics;
    assert(ttfp_get_subscript_metrics(face, &script_metrics));
    assert(script_metrics.x_size == 650);
    assert(script_metrics.y_size == 600);
    assert(script_metrics.x_offset == 0);
    assert(script_metrics.y_offset == 75);

    assert(ttfp_get_superscript_metrics(face, &script_metrics));
    assert(script_metrics.x_size == 650);
    assert(script_metrics.y_size == 600);
    assert(script_metrics.x_offset == 0);
    assert(script_metrics.y_offset == 350);

    ttfp_rect a_bbox = {0};
    assert(ttfp_get_glyph_bbox(face, a_gid, &a_bbox));
    assert(a_bbox.x_min == 3);
    assert(a_bbox.y_min == 0);
    assert(a_bbox.x_max == 541);
    assert(a_bbox.y_max == 656);

    assert(!ttfp_get_glyph_bbox(face, 0xFFFF, &a_bbox));

    uint32_t counter = 0;
    ttfp_outline_builder builder;
    builder.move_to = move_to_cb;
    builder.line_to = line_to_cb;
    builder.quad_to = quad_to_cb;
    builder.curve_to = curve_to_cb;
    builder.close_path = close_path_cb;
    assert(ttfp_outline_glyph(face, builder, &counter, a_gid, &a_bbox));
    assert(counter == 20);
    // The same as via ttfp_get_glyph_bbox()
    assert(a_bbox.x_min == 3);
    assert(a_bbox.y_min == 0);
    assert(a_bbox.x_max == 541);
    assert(a_bbox.y_max == 656);

    char glyph_name[256];
    assert(ttfp_get_glyph_name(face, a_gid, glyph_name));
    assert(strcmp(glyph_name, "A") == 0);

    free(font_data);

    return 0;
}
