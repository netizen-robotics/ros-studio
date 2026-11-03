/*
 * Copyright (c) 2018, Bosch Software Innovations GmbH.
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions are met:
 *
 *     * Redistributions of source code must retain the above copyright
 *       notice, this list of conditions and the following disclaimer.
 *     * Redistributions in binary form must reproduce the above copyright
 *       notice, this list of conditions and the following disclaimer in the
 *       documentation and/or other materials provided with the distribution.
 *     * Neither the name of the copyright holder nor the names of its contributors
 *       may be used to endorse or promote products derived from
 *       this software without specific prior written permission.
 *
 * THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
 * AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
 * IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
 * ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE
 * LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
 * CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
 * SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
 * INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
 * CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
 * ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
 * POSSIBILITY OF SUCH DAMAGE.
 */
//! Byte-indexed nonbinary RViz2 map/costmap/raw palette semantics.
//! Adapted from the pinned RViz source; binary distribution notice is in
//! crates/visualization/notices/rviz-map-palettes.txt.
use super::map_appearance::Palette;

pub fn palette(palette: Palette) -> [[u8; 4]; 256] {
    std::array::from_fn(|index| {
        let i = index as u32;
        match palette {
            Palette::Raw => [i as u8, i as u8, i as u8, 255],
            Palette::Map | Palette::Costmap => match i {
                255 => [0x70, 0x89, 0x86, 255],
                128..=254 => [255, (255 * (i - 128) / 126) as u8, 0, 255],
                101..=127 => [0, 255, 0, 255],
                _ if palette == Palette::Map => {
                    let gray = (255 - 255 * i / 100) as u8;
                    [gray, gray, gray, 255]
                }
                0 => [0, 0, 0, 0],
                99 => [0, 255, 255, 255],
                100 => [255, 0, 255, 255],
                _ => {
                    let red = (255 * i / 100) as u8;
                    [red, 0, 255 - red, 255]
                }
            },
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_rviz_endpoints_and_every_signed_cell_have_exact_colors() {
        let map = palette(Palette::Map);
        let cost = palette(Palette::Costmap);
        let raw = palette(Palette::Raw);
        for i in 0..256 {
            assert_eq!(raw[i], [i as u8, i as u8, i as u8, 255]);
            if i <= 100 {
                let gray = (255 - 255 * i / 100) as u8;
                assert_eq!(map[i], [gray, gray, gray, 255]);
            }
            if (101..128).contains(&i) {
                assert_eq!(map[i], [0, 255, 0, 255]);
                assert_eq!(cost[i], map[i]);
            }
            if (128..255).contains(&i) {
                assert_eq!(map[i], [255, (255 * (i - 128) / 126) as u8, 0, 255]);
                assert_eq!(cost[i], map[i]);
            }
        }
        assert_eq!(map[255], [112, 137, 134, 255]);
        assert_eq!(cost[255], map[255]);
        assert_eq!(cost[0], [0, 0, 0, 0]);
        assert_eq!(cost[1], [2, 0, 253, 255]);
        assert_eq!(cost[98], [249, 0, 6, 255]);
        assert_eq!(cost[99], [0, 255, 255, 255]);
        assert_eq!(cost[100], [255, 0, 255, 255]);
    }
}
