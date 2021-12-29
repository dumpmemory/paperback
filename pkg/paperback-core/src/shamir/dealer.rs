/*
 * paperback: paper backup generator suitable for long-term storage
 * Copyright (C) 2018-2020 Aleksa Sarai <cyphar@cyphar.com>
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

use crate::shamir::{
    gf::{self, EvaluablePolynomial, GfBarycentric, GfElem, GfElemPrimitive, GfPolynomial},
    shard::Shard,
    Error,
};

use std::mem;

/// Factory to share a secret using [Shamir Secret Sharing][sss].
///
/// [sss]: https://en.wikipedia.org/wiki/Shamir%27s_Secret_Sharing
#[derive(Clone, Debug)]
pub struct Dealer {
    polys: Vec<Box<dyn EvaluablePolynomial>>,
    secret_len: usize,
    threshold: GfElemPrimitive,
}

impl Dealer {
    /// Returns the number of *unique* `Shard`s generated by this `Dealer`
    /// required to recover the stored secret.
    #[allow(dead_code)]
    pub fn threshold(&self) -> u32 {
        self.threshold
    }

    /// Construct a new `Dealer` to shard the `secret`, requiring at least
    /// `threshold` shards to reconstruct the secret.
    pub fn new<B: AsRef<[u8]>>(threshold: u32, secret: B) -> Self {
        assert!(threshold > 0, "must at least have a threshold of one");
        let k = threshold - 1;
        let secret = secret.as_ref();
        let polys = secret
            // Generate &[u32] from &[u8], by chunking into sets of four.
            .chunks(mem::size_of::<GfElemPrimitive>())
            .map(GfElem::from_bytes)
            // Generate a random polynomial with the value as the constant.
            .map(|x0| {
                let mut poly = GfPolynomial::new_rand(k, &mut rand::thread_rng());
                *poly.constant_mut() = x0;
                Box::new(poly) as Box<dyn EvaluablePolynomial>
            })
            .collect::<Vec<_>>();
        Dealer {
            polys,
            threshold,
            secret_len: secret.len(),
        }
    }

    /// Get the secret value stored by the `Dealer`.
    pub fn secret(&self) -> Vec<u8> {
        self.polys
            .iter()
            .map(|poly| poly.constant())
            .flat_map(|x| x.to_bytes())
            .take(self.secret_len)
            .collect::<Vec<_>>()
    }

    /// Generate a new `Shard` for the secret.
    ///
    /// NOTE: The `x` value is calculated randomly, which means that there is a
    ///       small chance that two separate calls to `Dealer::shard` will
    ///       generate the same `Shard`. It is up to the caller to be sure that
    ///       they have enough *unique* shards to reconstruct the secret.
    // TODO: I'm not convinced the chances of collision are low enough...
    pub fn next_shard(&self) -> Shard {
        let mut x = GfElem::ZERO;
        while x == GfElem::ZERO {
            x = GfElem::new_rand(&mut rand::thread_rng());
        }
        self.shard(x).expect("non x=0 shard should've been created")
    }

    /// Generate a `Shard` for the secret using the given `x` value.
    fn shard(&self, x: GfElem) -> Option<Shard> {
        if x == GfElem::ZERO {
            return None;
        }
        let ys = self
            .polys
            .iter()
            .map(|poly| {
                let y = poly.evaluate(x);
                assert!(self.threshold == 1 || y != poly.constant());
                y
            })
            .collect::<Vec<_>>();
        Some(Shard {
            x,
            ys,
            threshold: self.threshold,
            secret_len: self.secret_len,
        })
    }

    /// Reconstruct an entire `Dealer` from a *unique* set of `Shard`s.
    ///
    /// The caller must pass exactly the correct number of shards.
    ///
    /// This operation is significantly slower than `recover_secret`, so it
    /// should only be used if it is necessary to construct additional shards
    /// with `Dealer::next_shard`.
    pub fn recover<S: AsRef<[Shard]>>(shards: S) -> Result<Self, Error> {
        let shards = shards.as_ref();
        assert!(!shards.is_empty(), "must be provided at least one shard");

        let threshold = shards[0].threshold();
        let polys_len = shards[0].ys.len();
        let secret_len = shards[0].secret_len;

        // TODO: Implement this consistency checking more nicely.
        for shard in shards {
            assert!(shard.threshold() == threshold, "shards must be consistent");
            assert!(shard.ys.len() == polys_len, "shards must be consistent");
            assert!(shard.secret_len == secret_len, "shards must be consistent");
        }

        assert!(
            shards.len() == threshold as usize,
            "must have exactly {} shards",
            threshold
        );

        let polys = (0..polys_len)
            .map(|i| {
                let xs = shards.iter().map(|s| s.x);
                let ys = shards.iter().map(|s| s.ys[i]);

                let points = xs.zip(ys).collect::<Vec<_>>();
                GfBarycentric::recover(threshold - 1, points.as_slice())
                    .map(|poly| Box::new(poly) as Box<dyn EvaluablePolynomial>)
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            polys,
            secret_len,
            threshold,
        })
    }
}

/// Reconstruct a secret from a set of `Shard`s.
///
/// This operation is significantly faster than `Dealer::recover`, so it should
/// always be used if the caller only needs to recover the secret.
/// `Dealer::recover` should only be used if the caller needs to create
/// additional shards with `Dealer::next_shard`.
pub fn recover_secret<S: AsRef<[Shard]>>(shards: S) -> Result<Vec<u8>, Error> {
    let shards = shards.as_ref();
    assert!(!shards.is_empty(), "must be provided at least one shard");

    let threshold = shards[0].threshold();
    let polys_len = shards[0].ys.len();
    let secret_len = shards[0].secret_len;

    // TODO: Implement this consistency checking more nicely.
    for shard in shards {
        assert!(shard.threshold() == threshold, "shards must be consistent");
        assert!(shard.ys.len() == polys_len, "shards must be consistent");
        assert!(shard.secret_len == secret_len, "shards must be consistent");
    }

    assert!(
        shards.len() == threshold as usize,
        "must have exactly {} shards",
        threshold
    );

    Ok((0..polys_len)
        .map(|i| {
            let xs = shards.iter().map(|s| s.x);
            let ys = shards.iter().map(|s| s.ys[i]);

            let points = xs.zip(ys).collect::<Vec<_>>();
            gf::lagrange_constant(threshold - 1, points.as_slice())
        })
        .collect::<Result<Vec<_>, _>>()? // XXX: I don't like this but flat_map() causes issues.
        .into_iter()
        .flat_map(|x| x.to_bytes())
        .take(secret_len)
        .collect::<Vec<_>>())
}

#[cfg(test)]
mod test {
    use super::*;

    use quickcheck::TestResult;

    // NOTE: We use u16s and u8s here (and limit the range) because generating
    //       ridiculously large dealers takes too long because of the amount of
    //       CSPRNG churn it causes. In principle we could have a special
    //       Dealer::new_inner() that takes a CoreRng but that's probably not
    //       necessary.

    #[quickcheck]
    fn basic_roundtrip(n: u16, secret: Vec<u8>) -> TestResult {
        if n < 1 || n > 4096 {
            return TestResult::discard();
        }
        let dealer = Dealer::new(n.into(), &secret);
        TestResult::from_bool(secret == dealer.secret())
    }

    #[quickcheck]
    fn recover_secret_fail(n: u8, secret: Vec<u8>) -> TestResult {
        // Invalid data. Note that large n values take a very long time to
        // recover the secret. This is proportional to secret.len(), which is
        // controlled by quickcheck and thus can be quite large.
        if n < 2 || n > 64 || secret.len() < 1 {
            return TestResult::discard();
        }

        let dealer = Dealer::new(n.into(), &secret);
        let shards = (0..(n - 1))
            .map(|_| {
                let mut shard = dealer.next_shard();
                shard.threshold -= 1;
                // Ensure shard IDs are always ID_LENGTH.
                assert_eq!(shard.id().len(), Shard::ID_LENGTH);
                shard
            })
            .collect::<Vec<_>>();

        TestResult::from_bool(recover_secret(shards).unwrap() != secret)
    }

    #[quickcheck]
    fn recover_secret_success(n: u8, secret: Vec<u8>) -> TestResult {
        // Invalid data. Note that large n values take a very long time to
        // recover the secret. This is proportional to secret.len(), which is
        // controlled by quickcheck and thus can be quite large.
        if n < 1 || n > 64 {
            return TestResult::discard();
        }

        let dealer = Dealer::new(n.into(), &secret);
        let shards = (0..n)
            .map(|_| {
                let shard = dealer.next_shard();
                // Ensure shard IDs are always ID_LENGTH.
                assert_eq!(shard.id().len(), Shard::ID_LENGTH);
                shard
            })
            .collect::<Vec<_>>();

        TestResult::from_bool(recover_secret(shards).unwrap() == secret)
    }

    #[quickcheck]
    fn limited_recover_success(n: u8, secret: Vec<u8>, test_xs: Vec<GfElem>) -> TestResult {
        // Invalid data. Note that even moderately large n values take a longer
        // time to fully recover -- which when paired with quickcheck makes it
        // take far too long. This is proportional to secret.len() (probably
        // O(n^2) with big constants or something like that).
        if n < 2 || n > 12 {
            return TestResult::discard();
        }
        let dealer = Dealer::new(n.into(), secret);
        let shards = (0..n)
            .map(|_| {
                let shard = dealer.next_shard();
                // Ensure shard IDs are always ID_LENGTH.
                assert_eq!(shard.id().len(), Shard::ID_LENGTH);
                shard
            })
            .collect::<Vec<_>>();
        let recovered_dealer = Dealer::recover(shards).unwrap();

        TestResult::from_bool(
            test_xs
                .iter()
                .all(|&x| dealer.shard(x) == recovered_dealer.shard(x)),
        )
    }
}
