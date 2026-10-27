cmake_minimum_required(VERSION 3.21)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
project(ZCB VERSION 3.6.18)

file(GLOB SOURCES src/*.cpp)
add_library(${PROJECT_NAME} SHARED ${SOURCES})

if (NOT DEFINED ENV{GEODE_SDK})
    message(FATAL_ERROR "Unable to find Geode SDK! Please define GEODE_SDK")
endif()
add_subdirectory($ENV{GEODE_SDK} ${CMAKE_CURRENT_BINARY_DIR}/geode)

if (ANDROID_ABI STREQUAL "arm64-v8a")
    set(ZCB_LIB ${CMAKE_CURRENT_SOURCE_DIR}/libs/android64/libzcb_android.a)
else()
    set(ZCB_LIB ${CMAKE_CURRENT_SOURCE_DIR}/libs/android32/libzcb_android.a)
endif()
target_link_libraries(${PROJECT_NAME} ${ZCB_LIB} EGL GLESv2 log dl m)
target_link_options(${PROJECT_NAME} PRIVATE -Wl,--allow-multiple-definition -Wl,--exclude-libs,ALL)

setup_geode_mod(${PROJECT_NAME})
