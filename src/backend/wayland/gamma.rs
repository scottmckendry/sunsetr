use anyhow::Result;

/// Convert color temperature to RGB gamma curves.
/// 
/// This function implements the color temperature to RGB conversion using
/// approximations of blackbody radiation curves, similar to implementations
/// in wlsunset, redshift, and other color temperature tools.
/// 
/// # Arguments
/// * `temperature` - Color temperature in Kelvin (1000-25000)
/// * `gamma_adjustment` - Gamma adjustment factor (0.1-10.0)
/// 
/// # Returns
/// Tuple of (red_factor, green_factor, blue_factor) for gamma curve adjustment
pub fn temperature_to_rgb(temperature: u32, gamma_adjustment: f32) -> (f32, f32, f32) {
    // Clamp temperature to reasonable bounds
    let temp = temperature.clamp(1000, 25000) as f64;
    
    let (red, green, blue) = if temp >= 6600.0 {
        // Daylight (6600K+): cool temperatures
        let red = temp / 100.0;
        let red = 329.698727446 * red.powf(-0.1332047592);
        
        let green = temp / 100.0;
        let green = 288.1221695283 * green.powf(-0.0755148492);
        
        let blue = 255.0;
        
        (red, green, blue)
    } else if temp >= 1000.0 {
        // Incandescent to daylight (1000K-6600K)
        let red = 255.0;
        
        let green = temp / 100.0;
        let green = if temp > 6600.0 {
            288.1221695283 * green.powf(-0.0755148492)
        } else {
            99.4708025861 * green.ln() - 161.1195681661
        };
        
        let blue = if temp >= 1900.0 {
            let blue_temp = temp / 100.0;
            138.5177312231 * blue_temp.ln() - 305.0447927307
        } else {
            0.0
        };
        
        (red, green, blue)
    } else {
        // Very warm temperatures (below 1000K)
        (255.0, 0.0, 0.0)
    };

    // Normalize to 0.0-1.0 range and apply gamma adjustment
    let red_factor = ((red / 255.0).clamp(0.0, 1.0) as f32).powf(1.0 / gamma_adjustment);
    let green_factor = ((green / 255.0).clamp(0.0, 1.0) as f32).powf(1.0 / gamma_adjustment);
    let blue_factor = ((blue / 255.0).clamp(0.0, 1.0) as f32).powf(1.0 / gamma_adjustment);

    (red_factor, green_factor, blue_factor)
}

/// Generate gamma table for a specific color channel.
/// 
/// Creates a gamma lookup table (LUT) that maps input values to output values
/// based on the gamma curve and color temperature adjustment.
/// 
/// # Arguments
/// * `size` - Size of the gamma table (typically 256)
/// * `gamma` - Gamma value for the curve
/// * `color_factor` - Color temperature adjustment factor (0.0-1.0)
/// 
/// # Returns
/// Vector of 16-bit gamma values for this color channel
pub fn generate_gamma_table(size: usize, gamma: f32, color_factor: f32) -> Vec<u16> {
    let mut table = Vec::with_capacity(size);
    
    for i in 0..size {
        let input = i as f32 / (size - 1) as f32;
        let gamma_corrected = input.powf(1.0 / gamma);
        let color_adjusted = gamma_corrected * color_factor;
        let output = (color_adjusted * 65535.0).clamp(0.0, 65535.0) as u16;
        table.push(output);
    }
    
    table
}

/// Create complete gamma tables for RGB channels.
/// 
/// Generates the full set of gamma lookup tables needed for the
/// wlr-gamma-control-unstable-v1 protocol.
/// 
/// # Arguments
/// * `size` - Size of each gamma table (reported by compositor)
/// * `temperature` - Color temperature in Kelvin
/// * `gamma` - Gamma adjustment value
/// 
/// # Returns
/// Byte vector containing concatenated R, G, B gamma tables
pub fn create_gamma_tables(size: usize, temperature: u32, gamma: f32) -> Result<Vec<u8>> {
    // Convert temperature to RGB factors
    let (red_factor, green_factor, blue_factor) = temperature_to_rgb(temperature, gamma);
    
    // Generate individual channel tables
    let red_table = generate_gamma_table(size, gamma, red_factor);
    let green_table = generate_gamma_table(size, gamma, green_factor);
    let blue_table = generate_gamma_table(size, gamma, blue_factor);
    
    // Convert to bytes (little-endian 16-bit values)
    let mut gamma_data = Vec::with_capacity(size * 3 * 2);
    
    // Red channel
    for value in red_table {
        gamma_data.extend_from_slice(&value.to_le_bytes());
    }
    
    // Green channel
    for value in green_table {
        gamma_data.extend_from_slice(&value.to_le_bytes());
    }
    
    // Blue channel
    for value in blue_table {
        gamma_data.extend_from_slice(&value.to_le_bytes());
    }
    
    Ok(gamma_data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_temperature_to_rgb_daylight() {
        let (r, g, b) = temperature_to_rgb(6500, 1.0);
        // Daylight should be fairly neutral
        assert!((r - 1.0).abs() < 0.1);
        assert!((g - 1.0).abs() < 0.1);
        assert!((b - 1.0).abs() < 0.1);
    }

    #[test]
    fn test_temperature_to_rgb_warm() {
        let (r, g, b) = temperature_to_rgb(3000, 1.0);
        // Warm light should be red-heavy, blue-light
        assert!(r > g);
        assert!(g > b);
        assert!(b < 0.5);
    }

    #[test]
    fn test_temperature_to_rgb_cool() {
        let (r, g, b) = temperature_to_rgb(8000, 1.0);
        // Cool light should be blue-heavy
        assert!(b > g);
        assert!(r < b);
    }

    #[test]
    fn test_gamma_table_generation() {
        let table = generate_gamma_table(256, 1.0, 1.0);
        assert_eq!(table.len(), 256);
        assert_eq!(table[0], 0);
        assert_eq!(table[255], 65535);
        
        // Should be monotonically increasing
        for i in 1..table.len() {
            assert!(table[i] >= table[i-1]);
        }
    }

    #[test]
    fn test_create_gamma_tables() {
        let tables = create_gamma_tables(256, 6500, 1.0).unwrap();
        // Should contain 3 channels * 256 entries * 2 bytes each
        assert_eq!(tables.len(), 256 * 3 * 2);
    }
} 