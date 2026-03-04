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

#include <assert.h>
#include <stdlib.h>
#include <string.h>

#include "svt_malloc.h"

#include "vector.h"
#define MAX(a, b) ((a) > (b) ? (a) : (b))

int svt_aom_vector_setup(Vector* vector, uint32_t capacity, uint32_t element_size) {
    assert(vector != NULL);

    if (vector == NULL) {
        return VECTOR_ERROR;
    }

    vector->size         = 0;
    vector->capacity     = MAX(VECTOR_MINIMUM_CAPACITY, capacity);
    vector->element_size = element_size;
    EB_MALLOC_NO_CHECK(vector->data, vector->capacity * element_size);

    return vector->data == NULL ? VECTOR_ERROR : VECTOR_SUCCESS;
}

int svt_aom_vector_destroy(Vector* vector) {
    assert(vector != NULL);

    if (vector == NULL) {
        return VECTOR_ERROR;
    }

    EB_FREE(vector->data);
    vector->data = NULL;

    return VECTOR_SUCCESS;
}

/* Insertion */
int svt_aom_vector_push_back(Vector* vector, void* element) {
    assert(vector != NULL);
    assert(element != NULL);

    if (_vector_should_grow(vector)) {
        if (_vector_adjust_capacity(vector) == VECTOR_ERROR) {
            return VECTOR_ERROR;
        }
    }

    _vector_assign(vector, vector->size, element);

    ++vector->size;

    return VECTOR_SUCCESS;
}

/* Information */

size_t svt_aom_vector_byte_size(const Vector* vector) {
    return (size_t)vector->size * vector->element_size;
}

/* Iterators */
Iterator svt_aom_vector_begin(Vector* vector) {
    return svt_aom_vector_iterator(vector, 0);
}

Iterator svt_aom_vector_iterator(Vector* vector, size_t index) {
    Iterator iterator = {NULL, 0};

    assert(vector != NULL && index <= vector->size);

    if (vector == NULL) {
        return iterator;
    }
    if (index > vector->size) {
        return iterator;
    }
    if (vector->element_size == 0) {
        return iterator;
    }

    iterator.pointer      = _vector_offset(vector, index);
    iterator.element_size = vector->element_size;

    return iterator;
}

void* svt_aom_iterator_get(Iterator* iterator) {
    return iterator->pointer;
}

void svt_aom_iterator_increment(Iterator* iterator) {
    assert(iterator != NULL);
    iterator->pointer = (unsigned char*)iterator->pointer + iterator->element_size;
}

/***** PRIVATE *****/

bool _vector_should_grow(Vector* vector) {
    assert(vector->size <= vector->capacity);
    return vector->size == vector->capacity;
}

void* _vector_offset(Vector* vector, size_t index) {
    return (unsigned char*)vector->data + (index * vector->element_size);
}

void _vector_assign(Vector* vector, size_t index, void* element) {
    /* Insert the element */
    void* offset = _vector_offset(vector, index);
    svt_memcpy(offset, element, vector->element_size);
}

int _vector_adjust_capacity(Vector* vector) {
    return _vector_reallocate(vector, MAX(1, vector->size * VECTOR_GROWTH_FACTOR));
}

int _vector_reallocate(Vector* vector, uint32_t new_capacity) {
    size_t new_capacity_in_bytes;
    void*  old;
    assert(vector != NULL);

    if (new_capacity < VECTOR_MINIMUM_CAPACITY) {
        if (vector->capacity > VECTOR_MINIMUM_CAPACITY) {
            new_capacity = VECTOR_MINIMUM_CAPACITY;
        } else {
            /* NO-OP */
            return VECTOR_SUCCESS;
        }
    }

    new_capacity_in_bytes = new_capacity * vector->element_size;
    old                   = vector->data;

    EB_MALLOC_NO_CHECK(vector->data, new_capacity_in_bytes);
    if (vector->data == NULL) {
        return VECTOR_ERROR;
    }
#ifdef __STDC_LIB_EXT1__
    if (memcpy_s(vector->data, new_capacity_in_bytes, old, svt_aom_vector_byte_size(vector)) != 0) {
        return VECTOR_ERROR;
    }
#else
    svt_memcpy(vector->data, old, svt_aom_vector_byte_size(vector));
#endif

    vector->capacity = new_capacity;

    EB_FREE(old);

    return VECTOR_SUCCESS;
}
