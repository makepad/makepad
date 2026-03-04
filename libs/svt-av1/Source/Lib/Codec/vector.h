/*
The MIT License(MIT)
Copyright(c) 2016 Peter Goldsborough

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files(the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions :

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
*/

#ifndef VECTOR_H
#define VECTOR_H

#include <stdbool.h>
#include <stddef.h>
#include "common_dsp_rtcd.h"
/***** DEFINITIONS *****/

#define VECTOR_MINIMUM_CAPACITY 2
#define VECTOR_GROWTH_FACTOR 2

#define VECTOR_ERROR -1
#define VECTOR_SUCCESS 0

#define VECTOR_UNINITIALIZED NULL
#define VECTOR_INITIALIZER {0, 0, 0, VECTOR_UNINITIALIZED}

/***** STRUCTURES *****/

typedef struct Vector {
    uint32_t size;
    uint32_t capacity;
    uint32_t element_size;
    void*    data;
} Vector;

typedef struct Iterator {
    void*  pointer;
    size_t element_size;
} Iterator;

/***** METHODS *****/

/* Constructor */
int svt_aom_vector_setup(Vector* vector, uint32_t capacity, uint32_t element_size);

/* Destructor */
int svt_aom_vector_destroy(Vector* vector);

/* Insertion */
int svt_aom_vector_push_back(Vector* vector, void* element);

/* Information */
size_t svt_aom_vector_byte_size(const Vector* vector);

/* Iterators */
Iterator svt_aom_vector_begin(Vector* vector);
Iterator svt_aom_vector_iterator(Vector* vector, size_t index);

void* svt_aom_iterator_get(Iterator* iterator);
#define ITERATOR_GET_AS(type, iterator) *((type*)svt_aom_iterator_get((iterator)))

void svt_aom_iterator_increment(Iterator* iterator);

/***** PRIVATE *****/

bool _vector_should_grow(Vector* vector);

void* _vector_offset(Vector* vector, size_t index);
void  _vector_assign(Vector* vector, size_t index, void* element);
int   _vector_adjust_capacity(Vector* vector);
int   _vector_reallocate(Vector* vector, uint32_t new_capacity);

#endif /* VECTOR_H */
