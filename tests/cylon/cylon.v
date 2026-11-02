// Mojo V3 Cylon scanner: one lit LED sweeps back and forth across the 8 LEDs.
// rst_n is the Mojo's reset button (active low).
module cylon (
    input  wire       clk,    // 50 MHz
    input  wire       rst_n,
    output wire [7:0] led
);
    // 2^22 cycles at 50 MHz = ~84 ms per step, ~1.2 s per full sweep.
    reg [21:0] tick = 0;
    reg [7:0]  pos  = 8'b0000_0001;
    reg        left = 1'b1;   // moving towards led[7]

    always @(posedge clk) begin
        if (!rst_n) begin
            tick <= 0;
            pos  <= 8'b0000_0001;
            left <= 1'b1;
        end else begin
            tick <= tick + 1'b1;
            if (&tick) begin
                if (left) begin
                    pos <= pos << 1;
                    if (pos[6]) left <= 1'b0;   // reaching led[7]: turn around
                end else begin
                    pos <= pos >> 1;
                    if (pos[1]) left <= 1'b1;   // reaching led[0]: turn around
                end
            end
        end
    end

    assign led = pos;
endmodule
