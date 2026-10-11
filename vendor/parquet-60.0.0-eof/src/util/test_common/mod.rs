// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

pub mod page_util;

#[cfg(test)]
pub mod file_util;

#[cfg(test)]
pub mod rand_gen;

/// PLENORA: the random generator of the unit tests, with a fixed seed in
/// place of `rand::rng()` (seeded from the system on every run: the data of
/// a test changed from one run to the next). Each call gives a new
/// generator, seeded from a counter of the calls made by the thread: every
/// test runs on its own thread, so the sequence of a test is the same on
/// every run, whatever the other tests do.
#[cfg(test)]
pub(crate) fn rng() -> rand::rngs::StdRng {
    use rand::SeedableRng;
    thread_local! {
        static CHIAMATE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }
    let chiamata = CHIAMATE.with(|c| {
        let n = c.get();
        c.set(n.wrapping_add(1));
        n
    });
    rand::rngs::StdRng::seed_from_u64(0x5EED_7E57_0000_0000 ^ chiamata)
}

#[cfg(test)]
mod tests {
    use super::rng;
    use rand::RngExt;

    /// PLENORA: the generator of the unit tests gives the same sequence on
    /// every thread, so on every run of a test (`rand::rng()` did not).
    #[test]
    fn test_rng_is_the_same_on_every_thread() {
        let sequenza = || {
            std::thread::spawn(|| {
                let mut primo = rng();
                let secondo: u64 = rng().random();
                (primo.random::<u64>(), secondo)
            })
            .join()
            .unwrap()
        };
        let (a, b) = sequenza();
        assert_eq!((a, b), sequenza());
        assert_ne!(a, b);
    }
}
